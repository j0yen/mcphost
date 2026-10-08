//! `mcphost submission-pack --chatgpt <dir>` (PRD-mcphost-chatgpt-submission-pack
//! P0 requirements 1-3): every artefact the ChatGPT Apps submission form
//! asks for, generated from the constants `registry-manifest` already
//! reads (`Cargo.toml`'s name/description, the public URL), so the pack,
//! `registry/server.json`, and the landing page's meta description can
//! never say different things.
//!
//! The icon is a dependency-free 1-bit indexed PNG drawn by [`icon_png`]
//! (stored deflate, no compression crate): 64x64 and well under the form's
//! 5,120-byte limit by construction; the size test is the contract.

use serde_json::{Value, json};

/// Form limits (the schema in [`SCHEMA_PATH`] repeats them for validation).
pub const MAX_NAME_CHARS: usize = 30;
pub const MAX_SHORT_DESCRIPTION_CHARS: usize = 160;
pub const MAX_ICON_BYTES: usize = 5_120;
pub const ICON_PX: u32 = 64;

/// The JSON Schema `metadata.json` is validated against, relative to the repo root.
pub const SCHEMA_PATH: &str = "docs/chatgpt-submission.schema.json";
/// Where the committed pack lives, relative to the repo root.
pub const PACK_PATH: &str = "submission/chatgpt";

/// Developer name on the listing (PRD open question; drafted: "mcphost").
pub const DEVELOPER_NAME: &str = "mcphost";

/// The landing page the meta description is read from. Compiled in so the
/// generator and the page can never be two copies of the same sentence.
pub const LANDING_PAGE: &str = include_str!("../www/index.html");

/// One generated file: pack-relative path plus exact bytes.
pub struct PackFile {
    pub path: &'static str,
    pub bytes: Vec<u8>,
}

/// The shared description: `Cargo.toml`'s `description`, which is also
/// `registry/server.json`'s `description` and the landing page's
/// `<meta name="description">`.
pub fn shared_description() -> &'static str {
    env!("CARGO_PKG_DESCRIPTION")
}

/// The `content` of the page's `<meta name="description">` tag.
pub fn landing_meta_description(html: &str) -> Option<String> {
    let at = html.find("<meta name=\"description\" content=\"")? + "<meta name=\"description\" content=\"".len();
    let end = html[at..].find('"')?;
    Some(html[at..at + end].to_string())
}

/// Reviewer notes: the ChatGPT connector steps are the install table's own
/// `chatgpt` row (never retyped here), then the credential-free first call.
pub fn reviewer_notes(public_url: &str) -> String {
    let base = public_url.trim_end_matches('/');
    let steps = crate::install_links::surface_for(base, "chatgpt").map(|s| s.artefact).unwrap_or_default();
    format!(
        "No test account or credential is needed. Follow these steps in order in a fresh ChatGPT session.\n\
         {steps}\n\
         The consent page creates a fresh workspace for the reviewer (first-grant signup); nothing is typed.\n\
         Then, in a new chat, call host.quickstart kind=echo. It returns a working echo tool and an onboarding block, again with no credential typed.\n"
    )
}

/// `metadata.json` content for `public_url`, with `description` as the
/// short description (a parameter so a drift test can change it).
pub fn build_metadata(public_url: &str, description: &str) -> Value {
    let base = public_url.trim_end_matches('/');
    json!({
        "name": env!("CARGO_PKG_NAME"),
        "short_description": description,
        "long_description": format!(
            "{description} Connect with one URL: no account form and no API key to paste."
        ),
        "developer_name": DEVELOPER_NAME,
        "website_url": base,
        "privacy_url": format!("{base}/privacy"),
        "terms_url": format!("{base}/terms"),
        "mcp_url": format!("{base}/mcp"),
        "auth": "oauth",
        "countries": "all",
        "icon": "icon-64.png",
        "reviewer_notes": reviewer_notes(base),
    })
}

fn screenshots_readme(public_url: &str) -> String {
    let base = public_url.trim_end_matches('/');
    format!(
        "# Screenshots to capture\n\n\
         Capture these three in a fresh ChatGPT session connected to {base}/mcp.\n\n\
         1. Prompt: `Call host.quickstart with kind=echo and show me the result.` Capture the tool result.\n\
         2. Prompt: `Use the echo tool to say hello.` Capture the reply.\n\
         3. Prompt: `Call host.whoami and tell me which workspace I am in.` Capture the reply.\n"
    )
}

/// The whole pack, rendered. `description` is the shared description.
pub fn render_pack(public_url: &str, description: &str) -> Vec<PackFile> {
    let mut metadata = serde_json::to_string_pretty(&build_metadata(public_url, description)).expect("serialize metadata");
    metadata.push('\n');
    vec![
        PackFile { path: "metadata.json", bytes: metadata.into_bytes() },
        PackFile { path: "icon-64.png", bytes: icon_png() },
        PackFile { path: "reviewer-notes.md", bytes: reviewer_notes(public_url).into_bytes() },
        PackFile { path: "screenshots/README.md", bytes: screenshots_readme(public_url).into_bytes() },
    ]
}

/// Writes `files` under `dir`, creating subdirectories.
pub fn write_pack(dir: &std::path::Path, files: &[PackFile]) -> std::io::Result<()> {
    for f in files {
        let path = dir.join(f.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, &f.bytes)?;
    }
    Ok(())
}

/// `--check`: pack-relative names of every file under `dir` that is missing
/// or differs from `files`. Empty means no drift.
pub fn check_pack(dir: &std::path::Path, files: &[PackFile]) -> Vec<&'static str> {
    files
        .iter()
        .filter(|f| std::fs::read(dir.join(f.path)).map(|on_disk| on_disk != f.bytes).unwrap_or(true))
        .map(|f| f.path)
        .collect()
}

// ---- icon ---------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
}

/// Distance from `(px, py)` to the segment `(ax, ay)-(bx, by)`.
fn segment_distance(px: f32, py: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32)) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// The landing page's terminal motif: a `>` prompt and a cursor bar, in the
/// page's own green on its own background (`www/index.html` palette).
pub fn icon_png() -> Vec<u8> {
    let side = ICON_PX as usize;
    let row_bytes = side / 8;
    let mut raw = Vec::with_capacity((row_bytes + 1) * side);
    for y in 0..side {
        raw.push(0); // filter: none
        let mut row = vec![0u8; row_bytes];
        for x in 0..side {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let chevron = segment_distance(fx, fy, (14.0, 12.0), (34.0, 32.0)) <= 3.5
                || segment_distance(fx, fy, (14.0, 52.0), (34.0, 32.0)) <= 3.5;
            let cursor = (38..=56).contains(&x) && (46..=51).contains(&y);
            if chevron || cursor {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        raw.extend_from_slice(&row);
    }
    let mut zlib = vec![0x78, 0x01, 0x01];
    let len = raw.len() as u16;
    zlib.extend_from_slice(&len.to_le_bytes());
    zlib.extend_from_slice(&(!len).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&ICON_PX.to_be_bytes());
    ihdr.extend_from_slice(&ICON_PX.to_be_bytes());
    ihdr.extend_from_slice(&[1, 3, 0, 0, 0]); // 1-bit, indexed, deflate, no filter, no interlace
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"PLTE", &[0x0b, 0x0d, 0x0e, 0x3f, 0xb9, 0x50]);
    chunk(&mut png, b"IDAT", &zlib);
    chunk(&mut png, b"IEND", &[]);
    png
}
