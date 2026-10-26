/// OAuth client values compiled into the binary. Real environment variables
/// (e.g. CI secrets) win over `src-tauri/.env`.
const ENV_KEYS: [&str; 2] = ["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET"];

fn main() {
    load_dotenv();
    tauri_build::build();
}

fn load_dotenv() {
    for key in ENV_KEYS {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let Ok(contents) = std::fs::read_to_string(".env") else {
        return;
    };
    println!("cargo:rerun-if-changed=.env");
    for line in contents.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if ENV_KEYS.contains(&key) && std::env::var(key).is_err() {
            let value = value.trim().trim_matches('"');
            println!("cargo:rustc-env={key}={value}");
        }
    }
}
