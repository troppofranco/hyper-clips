//! hc-sign — minisign key generation and signing for releases.
//!
//!   hc-sign keygen <out-dir>
//!       Creates hyperclips.pub, hyperclips.pub.b64 (for the app) and an
//!       encrypted hyperclips.key; prints the password once. Keep the secret
//!       key OUT of the repository (store it as a GitHub Actions secret).
//!   hc-sign sign <file> <version>
//!       Reads MINISIGN_SECRET_KEY (key file contents) and MINISIGN_PASSWORD
//!       from the environment, writes <file>.minisig with the trusted
//!       comment "hyperclips <version>".

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let res = match args.first().map(String::as_str) {
        Some("keygen") if args.len() >= 2 => keygen(&args[1]),
        Some("sign") if args.len() >= 3 => sign(&args[1], &args[2]),
        _ => Err("usage: hc-sign keygen <dir> | hc-sign sign <file> <version>".into()),
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hc-sign: {e}");
            ExitCode::FAILURE
        }
    }
}

fn random_password() -> String {
    // 32 bytes from the OS RNG via a throwaway key, hex encoded.
    let kp = minisign::KeyPair::generate_unencrypted_keypair().map_err(|e| e.to_string());
    let seed = kp.map(|k| k.pk.to_base64()).unwrap_or_default();
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{}{t:x}",
        seed.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
    )
}

fn keygen(dir: &str) -> Result<(), String> {
    let dir = std::path::Path::new(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let password = random_password();
    let kp = minisign::KeyPair::generate_encrypted_keypair(Some(password.clone()))
        .map_err(|e| e.to_string())?;
    let pk_box = kp.pk.to_box().map_err(|e| e.to_string())?.to_string();
    let sk_box = kp
        .sk
        .to_box(Some("hyper clips release key"))
        .map_err(|e| e.to_string())?
        .to_string();
    std::fs::write(dir.join("hyperclips.pub"), &pk_box).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("hyperclips.pub.b64"), kp.pk.to_base64()).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("hyperclips.key"), &sk_box).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("hyperclips.password"), &password).map_err(|e| e.to_string())?;
    println!("keys written to {}", dir.display());
    Ok(())
}

fn sign(file: &str, version: &str) -> Result<(), String> {
    let sk_text =
        std::env::var("MINISIGN_SECRET_KEY").map_err(|_| "MINISIGN_SECRET_KEY not set")?;
    let password = std::env::var("MINISIGN_PASSWORD").map_err(|_| "MINISIGN_PASSWORD not set")?;
    let sk = minisign::SecretKeyBox::from_string(&sk_text)
        .map_err(|e| e.to_string())?
        .into_secret_key(Some(password))
        .map_err(|e| e.to_string())?;
    let data = std::fs::read(file).map_err(|e| e.to_string())?;
    let comment = format!("hyperclips {}", version.trim_start_matches('v'));
    let sig = minisign::sign(
        None,
        &sk,
        std::io::Cursor::new(data),
        Some(&comment),
        Some("signature from hyper clips release key"),
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(format!("{file}.minisig"), sig.to_string()).map_err(|e| e.to_string())?;
    println!("signed {file} ({comment})");
    Ok(())
}
