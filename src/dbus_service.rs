use crossbeam_channel::Sender;
use zbus::blocking::Connection;
use zbus::interface;

use crate::config::AppCommand;

struct VoiceToTextService {
    cmd_tx: Sender<AppCommand>,
}

#[interface(name = "org.voicetotext.Daemon")]
impl VoiceToTextService {
    fn toggle(&self) -> String {
        let _ = self.cmd_tx.send(AppCommand::ToggleRecording);
        "ok".into()
    }

    fn quit(&self) -> String {
        let _ = self.cmd_tx.send(AppCommand::Quit);
        "ok".into()
    }
}

/// Start the D-Bus server. The returned Connection must be kept alive for the
/// duration of the program — dropping it unregisters the bus name.
pub fn start_server(cmd_tx: Sender<AppCommand>) -> Result<Connection, Box<dyn std::error::Error>> {
    let conn = zbus::blocking::connection::Builder::session()?
        .name("org.voicetotext.Daemon")?
        .serve_at("/org/voicetotext/Daemon", VoiceToTextService { cmd_tx })?
        .build()?;

    eprintln!("D-Bus service registered: org.voicetotext.Daemon");
    Ok(conn)
}

/// Send a method call to the running daemon.
pub fn send_command(method: &str) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::session()?;
    conn.call_method(
        Some("org.voicetotext.Daemon"),
        "/org/voicetotext/Daemon",
        Some("org.voicetotext.Daemon"),
        method,
        &(),
    )?;
    Ok(())
}
