use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    pin::Pin,
    thread,
};

use lazy_static::lazy_static;
use regex::Regex;

const GET: &'static str = "GET";
const POST: &'static str = "POST";

// const PUT: &'static str = "PUT";
// const UPDATE: &'static str = "UPDATE";
// const DELETE: &'static str = "DELETE";
// const HEAD: &'static str = "HEAD";
// const CONNECT: &'static str = "CONNECT";
// const OPTIONS: &'static str = "OPTIONS";
// const TRACE: &'static str = "TRACE";
// const PATCH: &'static str = "PATCH";

const USER_AGENT_PATH: &'static str = "user-agent";
const FILES_PATH: &'static str = "files";
const DIR_PATH: &'static str = "--directory";

lazy_static! {
    static ref USER_AGENT_RE: Regex = Regex::new(r"User-Agent:\s*([^\r\n]*)").unwrap();
    static ref ECHO_RE: Regex = Regex::new(r"echo/([^\s\r\n]*)").unwrap();
    static ref FILE_NAME_RE: Regex = Regex::new(r"files/([^\s\r\n]*)").unwrap();
    static ref METHOD_RE: Regex = Regex::new(r"^(.*)\s+/.*\s+HTTP/1\.1").unwrap();
    static ref PATH_RE: Regex = Regex::new(r".*\s+/(.*)\s+HTTP/1\.1").unwrap();
    static ref HEADERS_RE: Regex = Regex::new(r"(.*?):\s*(.*)\s*").unwrap();
}

#[allow(dead_code)]
enum HttpResponse {
    Ok(Option<String>),
    OkStream(Option<Vec<u8>>),
    NotFound,
    Created,
}

trait IntoResponse {
    fn into_response(&self) -> String;
}
trait IntoStreamResponse {
    fn into_stream_response(&self) -> Vec<u8>;
}

impl IntoStreamResponse for HttpResponse {
    fn into_stream_response(&self) -> Vec<u8> {
        match self {
            HttpResponse::OkStream(Some(body)) => {
                let content_length = body.len();
                let response_headers = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
                        content_length,
                    );
                [response_headers.as_bytes().to_vec(), body.to_owned()].concat()
            }
            _ => self.into_response().as_bytes().to_vec(),
        }
    }
}
impl IntoResponse for HttpResponse {
    fn into_response(&self) -> String {
        match self {
            HttpResponse::Ok(body) => {
                return match body {
                    Some(body) => {
                        let content_length = body.as_bytes().len();
                        format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
                        content_length,
                        body
                    )
                    }
                    None => format!("HTTP/1.1 200 OK\r\n\r\n"),
                }
            }
            HttpResponse::NotFound => format!("HTTP/1.1 404 NOT FOUND\r\n\r\n"),
            HttpResponse::Created => format!("HTTP/1.1 201 CREATED\r\n\r\n"),
            _ => String::default(),
        }
    }
}

fn extract_path_echo<T>(s: &T) -> Option<String>
where
    T: AsRef<str>,
{
    let string = s.as_ref();
    let caps = ECHO_RE.captures(string)?;
    let matching = caps.get(1)?;
    Some(matching.as_str().to_string())
}
fn extract_path_filename<T>(s: &T) -> Option<String>
where
    T: AsRef<str>,
{
    let string = s.as_ref();
    let caps = FILE_NAME_RE.captures(string)?;
    let matching = caps.get(1)?;
    Some(matching.as_str().to_string())
}
fn extract_path<T>(s: &T) -> Option<String>
where
    T: AsRef<str>,
{
    let string = s.as_ref();
    let caps = PATH_RE.captures(string)?;
    let matching = caps.get(1)?;
    Some(matching.as_str().to_string())
}

#[derive(Debug)]
enum TypedHeader {
    Number(i32),
    Str(String),
}
struct HttpRequest<'a> {
    method: String,
    path: String,
    headers: HashMap<String, TypedHeader>,
    body: Option<Pin<&'a [u8]>>,
}

trait FromStr {
    fn from_str<T>(s: &T) -> Option<Self>
    where
        T: AsRef<str>,
        Self: Sized;
}

impl<'a> HttpRequest<'a> {
    fn with_body(mut self, body: &'a [u8]) -> Self {
        self.body = Some(Pin::new(body));
        return self;
    }
}

impl<'a> FromStr for HttpRequest<'a> {
    fn from_str<T>(s: &T) -> Option<Self>
    where
        T: AsRef<str>,
    {
        let string = s.as_ref();
        let caps = METHOD_RE.captures(string)?;
        let matching = caps.get(1)?;
        let method = matching.as_str().to_string();
        let path = extract_path(s)?;
        let mut headers = HashMap::new();
        for cap in HEADERS_RE.captures_iter(string) {
            if let (Some(key_match), Some(value_match)) = (cap.get(1), cap.get(2)) {
                let key = key_match.as_str().trim();
                let value = value_match.as_str().trim();

                headers.insert(
                    key.to_string(),
                    if let Ok(int_value) = value.parse::<i32>() {
                        TypedHeader::Number(int_value)
                    } else {
                        TypedHeader::Str(value.to_string())
                    },
                );
            }
        }

        Some(HttpRequest {
            method,
            path,
            headers,
            body: None,
        })
    }
}
fn extract_user_agent<T>(s: &T) -> Option<String>
where
    T: AsRef<str>,
{
    let string = s.as_ref();
    let caps = USER_AGENT_RE.captures(string)?;
    let matching = caps.get(1)?;
    Some(matching.as_str().to_string())
}

fn get_arg(a: &'static str) -> Option<String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg.as_str() == a {
            return args.next();
        }
    }
    None
}

fn file_contents(path: &PathBuf) -> Result<Vec<u8>, std::io::Error> {
    return match std::fs::metadata(&path) {
        Ok(metadata) => {
            if metadata.is_file() {
                let mut file = File::open(path)?;
                let mut contents = Vec::new();
                file.read_to_end(&mut contents)?;
                Ok(contents)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("the file was not found at path {:?}", path),
                ))
            }
        }
        Err(err) => Err(err),
    };
}

fn write_file(path: &PathBuf, data: &[u8]) -> Result<usize, std::io::Error> {
    let mut new_file = File::create(path)?;
    new_file.write_all(data).unwrap();
    new_file.flush().unwrap();
    Ok(data.len())
}

fn process_stream(stream: &mut TcpStream) -> io::Result<(Vec<u8>, usize)> {
    let mut buffer = Vec::new();
    let mut temp_buf = [0; 1024]; // Temporary buffer for each read
    let mut _body_start = 0;
    loop {
        let bytes_read = stream.read(&mut temp_buf)?;
        if bytes_read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "End of stream",
            ));
        }
        buffer.extend_from_slice(&temp_buf[..bytes_read]);

        if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            _body_start = pos + 4; // The body starts after the "\r\n\r\n"
            break;
        }
    }
    Ok((buffer, _body_start))
}

fn main() {
    println!("Logs from program will appear here!");
    let listener = TcpListener::bind("127.0.0.1:4221").unwrap();
    for stream in listener.incoming() {
        // Stage 6 (Multi connection server)
        thread::spawn(|| {
            match stream {
                Ok(mut stream) => {
                    println!("Accepted new connection"); // Stage 1
                    if let Ok((buf, body_pos)) = process_stream(&mut stream) {
                        let req_str = String::from_utf8_lossy(&buf[..body_pos]);
                        let mut response = HttpResponse::NotFound;

                        match HttpRequest::from_str(&req_str)
                            .map(|req| req.with_body(&buf[body_pos..]))
                        {
                            Some(HttpRequest {
                                method,
                                path,
                                headers,
                                body,
                            }) => {

                                if method == GET {
                                    if path.is_empty() {
                                        // Stage 2
                                        response = HttpResponse::Ok(None);
                                    } else if let Some(echo) = extract_path_echo(&path) {
                                        // Stage 4
                                        response = HttpResponse::Ok(Some(echo));
                                    } else if path == USER_AGENT_PATH {
                                        match headers.get("User-Agent") {
                                            Some(TypedHeader::Str(user_agent)) => {
                                                response =
                                                    HttpResponse::Ok(Some(user_agent.to_string()));
                                            }
                                            _ => {}
                                        }
                                    } else if path.contains(FILES_PATH) {
                                        // Stage 7
                                        if let (Some(dir_name), Some(file_name)) =
                                            (get_arg(DIR_PATH), extract_path_filename(&path))
                                        {
                                            let mut file_path = PathBuf::from(dir_name);
                                            file_path.push(&file_name);
                                            if let Ok(contents) = file_contents(&file_path) {
                                                response = HttpResponse::OkStream(Some(contents));
                                            }
                                        }
                                    }
                                } else if method == POST {
                                    // Stage 8
                                    if path.contains(FILES_PATH) {
                                        if let (Some(dir_name), Some(file_name), Some(data)) =
                                            (get_arg(DIR_PATH), extract_path_filename(&path), body)
                                        {
                                            let mut file_path = PathBuf::from(dir_name);
                                            file_path.push(&file_name);
                                            if let Ok(_written_bytes) =
                                                write_file(&file_path, &data)
                                            {
                                                response = HttpResponse::Created;
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {
                                println!("Server does not support the http request {req_str}");
                            }
                        };
                        match response {
                            HttpResponse::OkStream(_) => {
                                stream.write_all(&response.into_stream_response()).unwrap();
                            }
                            HttpResponse::Ok(_) | HttpResponse::Created => {
                                stream
                                    .write_all(&response.into_response().as_bytes())
                                    .unwrap();
                            }
                            _ => {
                                // Stage 3 - Not found
                                stream
                                    .write_all(HttpResponse::NotFound.into_response().as_bytes())
                                    .unwrap();
                            }
                        }
                        stream.flush().unwrap(); // Flush the stream
                    }
                }
                Err(e) => {
                    println!("error: {}", e);
                }
            }
        });
    }
}
