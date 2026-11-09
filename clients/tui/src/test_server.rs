// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! A canned HTTP server for tests of the blocking client and commands.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::mpsc,
};

/// Serves one canned `(status, json)` response per connection, in order,
/// and reports each request's line and headers, lowercased. Returns the
/// base URL.
pub fn server(responses: Vec<(u16, &str)>) -> (String, mpsc::Receiver<String>) {
    let responses: Vec<(u16, String)> = responses
        .into_iter()
        .map(|(status, body)| (status, body.to_owned()))
        .collect();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for (status, body) in responses {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let line = line.trim_end().to_lowercase();
                if line.is_empty() {
                    break;
                }
                if let Some(n) = line.strip_prefix("content-length: ") {
                    length = n.parse().unwrap();
                }
                head.push_str(&line);
                head.push('\n');
            }
            reader.read_exact(&mut vec![0; length]).unwrap();
            // The test may have stopped listening; that is not our failure.
            let _ = tx.send(head);
            write!(
                reader.get_mut(),
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (base, rx)
}
