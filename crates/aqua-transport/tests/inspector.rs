use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use aqua_transport::{FrameRecord, Inspector, InspectorError, InspectorServer};

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

fn connect(server: &InspectorServer) -> (TcpStream, BufReader<TcpStream>) {
    let stream = TcpStream::connect(server.local_addr()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let reader = BufReader::new(stream.try_clone().unwrap());
    (stream, reader)
}

fn ask(stream: &mut TcpStream, reader: &mut BufReader<TcpStream>, command: &str) -> String {
    writeln!(stream, "{command}").unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    line
}

fn record(frame: u64) -> FrameRecord {
    FrameRecord {
        frame,
        extent: (1280, 720),
        passes: 3,
        instances: 40,
        gpu_micros: Some(210),
        ..FrameRecord::default()
    }
}

#[test]
fn stats_and_tree_answer_with_one_json_line_each() {
    let inspector = Inspector::new();
    inspector.update(|s| {
        s.frames_presented = 7;
        s.tracks = 2;
        s.last = Some(record(7));
        s.last_error = Some("a \"quoted\"\nerror".into());
        s.tree_json = Some("{\"role\":\"window\"}".into());
    });
    let server = InspectorServer::bind(inspector, loopback()).unwrap();
    let (mut stream, mut reader) = connect(&server);

    let stats = ask(&mut stream, &mut reader, "stats");
    assert!(stats.contains("\"frames_presented\":7"), "{stats}");
    assert!(stats.contains("\"gpu_micros\":210"), "{stats}");
    assert!(stats.contains("\"width\":1280"), "{stats}");
    assert!(stats.contains("a \\\"quoted\\\"\\nerror"), "{stats}");
    assert!(stats.ends_with("}\n"));

    let tree = ask(&mut stream, &mut reader, "tree");
    assert_eq!(tree, "{\"tree\":{\"role\":\"window\"}}\n");

    let unknown = ask(&mut stream, &mut reader, "dance");
    assert!(unknown.contains("unknown command"));
}

#[test]
fn watch_streams_a_line_per_published_change() {
    let inspector = Inspector::new();
    let server = InspectorServer::bind(inspector.clone(), loopback()).unwrap();
    let (mut stream, mut reader) = connect(&server);
    writeln!(stream, "watch").unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("\"frames_presented\":0"), "{line}");

    for frame in 1..=3u64 {
        inspector.update(|s| {
            s.frames_presented = frame;
            s.last = Some(record(frame));
        });
        std::thread::sleep(Duration::from_millis(60));
    }
    let mut seen = Vec::new();
    while seen.len() < 3 {
        line.clear();
        reader.read_line(&mut line).unwrap();
        seen.push(line.clone());
    }
    assert!(seen.last().unwrap().contains("\"frames_presented\":3"));
    assert!(seen.windows(2).all(|pair| pair[0] != pair[1]));
}

#[test]
fn only_loopback_addresses_are_accepted() {
    let any: SocketAddr = "0.0.0.0:0".parse().unwrap();
    match InspectorServer::bind(Inspector::new(), any) {
        Err(InspectorError::NotLoopback(addr)) => assert_eq!(addr, any),
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("a public address must be refused"),
    }
}

#[test]
fn dropping_the_server_closes_open_connections() {
    let server = InspectorServer::bind(Inspector::new(), loopback()).unwrap();
    let (mut stream, mut reader) = connect(&server);
    assert!(ask(&mut stream, &mut reader, "stats").contains("frames_presented"));
    drop(server);
    let mut line = String::new();
    assert_eq!(reader.read_line(&mut line).unwrap(), 0);
}
