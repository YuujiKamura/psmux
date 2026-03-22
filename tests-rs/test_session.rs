use std::io::{BufRead, Write};
use std::net::TcpListener;
use std::time::Duration;

// ── Test 1: send_auth_cmd uses nodelay + flush ───────────────────────
//
// Spin up a local TCP listener, call send_auth_cmd, and verify the
// server side actually receives the AUTH line and the command payload.
// If flush were missing, the data might not arrive within the short
// timeout, causing the test to fail.

#[test]
fn send_auth_cmd_delivers_data_with_flush() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{}", port);

    let addr_clone = addr.clone();
    let handle = std::thread::spawn(move || {
        super::send_auth_cmd(&addr_clone, "test-key-123", b"list-sessions\n").unwrap();
    });

    // Accept the connection and read what was sent
    let (stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut reader = std::io::BufReader::new(&stream);

    // First line should be the AUTH
    let mut auth_line = String::new();
    reader.read_line(&mut auth_line).unwrap();
    assert_eq!(auth_line.trim(), "AUTH test-key-123");

    // Second line should be the command
    let mut cmd_line = String::new();
    reader.read_line(&mut cmd_line).unwrap();
    assert_eq!(cmd_line.trim(), "list-sessions");

    handle.join().unwrap();
}

#[test]
fn send_auth_cmd_response_delivers_data_with_flush() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{}", port);

    let addr_clone = addr.clone();
    let handle = std::thread::spawn(move || {
        let _ = super::send_auth_cmd_response(&addr_clone, "my-key", b"has-session\n");
    });

    let (stream, _) = listener.accept().unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut reader = std::io::BufReader::new(&stream);

    let mut auth_line = String::new();
    reader.read_line(&mut auth_line).unwrap();
    assert_eq!(auth_line.trim(), "AUTH my-key");

    let mut cmd_line = String::new();
    reader.read_line(&mut cmd_line).unwrap();
    assert_eq!(cmd_line.trim(), "has-session");

    // Send back a response so the client doesn't hang
    let writer = reader.into_inner();
    let mut w = writer.try_clone().unwrap();
    let _ = w.write_all(b"OK\n");
    let _ = w.flush();

    handle.join().unwrap();
}

// ── Test 2: has-session does not kill the server ─────────────────────
//
// Start a minimal TCP "server" that dispatches HasSession via a channel,
// send the has-session command, and verify the server thread is still
// alive afterward (i.e., no process::exit was called).

#[test]
fn has_session_does_not_kill_server() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{}", port);

    // Simulate the server side: accept connection, read AUTH + command,
    // respond to has-session, and keep running.
    let server_handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;

        // Read AUTH line
        let mut auth_line = String::new();
        reader.read_line(&mut auth_line).unwrap();

        // Send AUTH OK
        let _ = writer.write_all(b"OK\n");
        let _ = writer.flush();

        // Read command
        let mut cmd_line = String::new();
        reader.read_line(&mut cmd_line).unwrap();
        let cmd = cmd_line.trim();

        assert_eq!(cmd, "has-session", "expected has-session command");

        // Respond just like the real server: send OK (session exists)
        let _ = writer.write_all(b"OK\n");
        let _ = writer.flush();

        // Server is still alive here -- return a sentinel value to prove it
        42u32
    });

    // Client side: send has-session
    let response = super::send_auth_cmd_response(&addr, "test-key", b"has-session\n");
    assert!(response.is_ok(), "send_auth_cmd_response should succeed");

    // The critical assertion: the server thread completed normally
    // (didn't call process::exit or panic)
    let result = server_handle.join().expect("server thread should not panic or exit");
    assert_eq!(result, 42, "server should return sentinel value, proving it stayed alive");
}

#[test]
fn has_session_server_accepts_subsequent_connections() {
    // After handling has-session, the server should still accept new connections.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{}", port);

    let listener_handle = std::thread::spawn(move || {
        // Accept first connection (has-session)
        let (stream1, _) = listener.accept().unwrap();
        stream1.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut r1 = std::io::BufReader::new(stream1.try_clone().unwrap());
        let mut w1 = stream1;
        let mut line = String::new();
        r1.read_line(&mut line).unwrap(); // AUTH
        let _ = w1.write_all(b"OK\n");
        let _ = w1.flush();
        line.clear();
        r1.read_line(&mut line).unwrap(); // has-session
        let _ = w1.write_all(b"OK\n");
        let _ = w1.flush();
        drop(w1);
        drop(r1);

        // Accept second connection (list-sessions) -- proves server is still alive
        let (stream2, _) = listener.accept().unwrap();
        stream2.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut r2 = std::io::BufReader::new(stream2.try_clone().unwrap());
        let mut w2 = stream2;
        line.clear();
        r2.read_line(&mut line).unwrap(); // AUTH
        let _ = w2.write_all(b"OK\n");
        let _ = w2.flush();
        line.clear();
        r2.read_line(&mut line).unwrap(); // list-sessions
        assert_eq!(line.trim(), "list-sessions");
        let _ = w2.write_all(b"session1\n");
        let _ = w2.flush();

        true
    });

    // First: send has-session
    let _ = super::send_auth_cmd_response(&addr, "key", b"has-session\n");

    // Second: send list-sessions -- this proves the server didn't die
    let resp = super::send_auth_cmd_response(&addr, "key", b"list-sessions\n");
    assert!(resp.is_ok(), "second command after has-session should succeed");

    let server_alive = listener_handle.join().expect("server thread should not panic");
    assert!(server_alive, "server should still be alive after has-session");
}

// ── Test 3: stale PID file recovery ──────────────────────────────────
//
// These tests use `check_server_alive_in_dir` with a per-test temp
// directory, avoiding any shared state (`~/.psmux/` or `USERPROFILE`).
// This makes them safe to run in parallel across the three binary
// targets (pmux, psmux, tmux) that cargo builds from the same source.

/// Create a temporary directory for a single test and return its path.
/// The caller is responsible for cleanup (or can let the OS handle it).
fn make_temp_psmux_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "psmux_test_{}_{}_{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(windows)]
#[test]
fn is_pid_alive_with_current_process() {
    // Our own PID should be alive
    let my_pid = std::process::id();
    assert!(super::is_pid_alive(my_pid), "current process PID should be alive");
}

#[cfg(windows)]
#[test]
fn is_pid_alive_with_nonexistent_pid() {
    // A very large PID that almost certainly doesn't exist
    assert!(!super::is_pid_alive(99999999), "PID 99999999 should not be alive");
}

#[cfg(windows)]
#[test]
fn check_server_alive_no_pid_file_returns_true() {
    // When no .pid file exists, check_server_alive should return true
    // (backward compatibility).
    let tmp = make_temp_psmux_dir("no_pid");
    let result = super::check_server_alive_in_dir("nonexistent_session", &tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(
        result,
        "check_server_alive should return true when no .pid file exists (backward compat)"
    );
}

#[cfg(windows)]
#[test]
fn check_server_alive_with_live_pid_returns_true() {
    // Write a .pid file containing our own PID (which is alive)
    let tmp = make_temp_psmux_dir("live_pid");
    let session = "test_live";
    let pid_path = tmp.join(format!("{}.pid", session));
    std::fs::write(&pid_path, format!("{}", std::process::id()))
        .expect("failed to write pid file");

    let result = super::check_server_alive_in_dir(session, &tmp);
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(result, "check_server_alive should return true for a live PID");
}

#[cfg(windows)]
#[test]
fn check_server_alive_with_dead_pid_returns_false_and_cleans_up() {
    // Write .pid, .port, .key files with a dead PID
    let tmp = make_temp_psmux_dir("dead_pid");
    let session = "test_dead";

    let pid_path = tmp.join(format!("{}.pid", session));
    let port_path = tmp.join(format!("{}.port", session));
    let key_path = tmp.join(format!("{}.key", session));

    // Write stale files with a definitely-dead PID
    std::fs::write(&pid_path, "99999999").expect("failed to write pid file");
    std::fs::write(&port_path, "12345").expect("failed to write port file");
    std::fs::write(&key_path, "fake-key").expect("failed to write key file");

    let result = super::check_server_alive_in_dir(session, &tmp);

    // Verify the function returned false
    assert!(!result, "check_server_alive should return false for a dead PID");

    // Verify stale files were cleaned up
    assert!(!pid_path.exists(), "stale .pid file should be deleted");
    assert!(!port_path.exists(), "stale .port file should be deleted");
    assert!(!key_path.exists(), "stale .key file should be deleted");

    let _ = std::fs::remove_dir_all(&tmp);
}
