//! Sends a file to a Discord webhook. Only runs when the user explicitly asks for it.

use hc_core::error::{Error, Result};
use std::path::Path;
use std::time::Duration;

/// Discord's current upload limit for webhooks on normal servers.
pub const MAX_UPLOAD_BYTES: u64 = 25 * 1024 * 1024;

/// Accepts only genuine Discord webhook URLs so a typo never sends a clip elsewhere.
pub fn validate_webhook(url: &str) -> Result<()> {
    let ok = [
        "https://discord.com/api/webhooks/",
        "https://discordapp.com/api/webhooks/",
    ]
    .iter()
    .any(|prefix| url.starts_with(prefix) && url.len() > prefix.len());
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid("not a Discord webhook URL".into()))
    }
}

fn clean_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// multipart/form-data body with one text field (`content`) and one file (`file`).
pub fn multipart_body(boundary: &str, content: &str, file_name: &str, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(data.len() + 512);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"content\"\r\n\r\n{content}\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n",
            clean_filename(file_name)
        )
        .as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

pub fn send_discord(webhook: &str, file: &Path, message: &str) -> Result<()> {
    validate_webhook(webhook)?;
    let size = std::fs::metadata(file)?.len();
    if size > MAX_UPLOAD_BYTES {
        return Err(Error::Invalid(format!(
            "file is {:.1} MB; Discord accepts up to {} MB (shrink it with the size optimizer)",
            size as f64 / 1_048_576.0,
            MAX_UPLOAD_BYTES / 1_048_576
        )));
    }
    let data = std::fs::read(file)?;
    let name = file.file_name().map_or_else(
        || "clip.mp4".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let boundary = "----HyperClipsBoundary7f3a9c";
    let body = multipart_body(boundary, message, &name, &data);
    let agent = ureq::AgentBuilder::new()
        .user_agent("HyperClips")
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build();
    agent
        .post(webhook)
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .send_bytes(&body)
        .map_err(|e| Error::System(format!("Discord upload failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_discord_webhooks_are_accepted() {
        assert!(validate_webhook("https://discord.com/api/webhooks/1/abc").is_ok());
        assert!(validate_webhook("https://discord.com/api/webhooks/").is_err());
        assert!(validate_webhook("https://evil.example/api/webhooks/1/abc").is_err());
        assert!(validate_webhook("http://discord.com/api/webhooks/1/abc").is_err());
    }

    #[test]
    fn multipart_contains_both_parts_and_sanitised_name() {
        let body = multipart_body("B", "hi", "my clip\".mp4", b"DATA");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"content\"\r\n\r\nhi"));
        assert!(text.contains("filename=\"my_clip_.mp4\""));
        assert!(text.contains("DATA"));
        assert!(text.ends_with("--B--\r\n"));
    }
}
