use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

// The container is embedded; these are exactly five local child fixture files.
pub const ROOT: &str = "<!doctype html><html><head><title>Nested frames</title></head><frameset id='layout' rows='65%,35%'><frame id='topFrame' name='topPane' title='Upper row' src='top.html'><frame id='bottomFrame' name='bottomPane' title='Lower row' src='bottom.html'></frameset></html>";
pub struct Server { port: u16 }
impl Server {
    pub fn url(&self,path:&str)->String {format!("http://127.0.0.1:{}{path}",self.port)}
}
pub fn serve()->Server {
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();
    let port=listener.local_addr().unwrap().port();
    std::thread::spawn(move||{
        for stream in listener.incoming() {
            let Ok(mut stream)=stream else {continue};
            std::thread::spawn(move||{
                let mut reader=BufReader::new(stream.try_clone().unwrap());
                let mut request=String::new();if reader.read_line(&mut request).unwrap_or(0)==0{return;}
                let path=request.split_whitespace().nth(1).unwrap_or("/");
                let response=match path {
                    "/"|"/nested_frames"=>ROOT,
                    "/top.html"=>include_str!("../pages/nested_frames/top.html"),
                    "/left.html"=>include_str!("../pages/nested_frames/left.html"),
                    "/middle.html"=>include_str!("../pages/nested_frames/middle.html"),
                    "/right.html"=>include_str!("../pages/nested_frames/right.html"),
                    "/bottom.html"=>include_str!("../pages/nested_frames/bottom.html"),
                    "/loop.html"=>"<!doctype html><frameset rows='*'><frame src='loop.html'></frameset>",
                    _=>"<!doctype html><body>Not found</body>",
                };
                loop {let mut header=String::new();if reader.read_line(&mut header).unwrap_or(0)==0{return;}if header.trim().is_empty(){break;}}
                let head=format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response.len());
                let _=stream.write_all(head.as_bytes()).and_then(|_|stream.write_all(response.as_bytes()));
            });
        }
    });
    Server{port}
}
