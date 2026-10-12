use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

use hubwriter::{
    Failure, Image, Progress, check_image, find, parse_sha256, platform, refusal, write_image,
};

const USAGE: &str = "usage: dd-hub-writer list
       dd-hub-writer write <image> <device-id> <sha256-hex> [--progress]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["list"] => list(),
        ["write", image, device, sha] => write(image, device, sha, false),
        ["write", image, device, sha, "--progress"] => write(image, device, sha, true),
        _ => Err(Failure::Other(USAGE.into())),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            eprintln!("dd-hub-writer: {}", f.message());
            ExitCode::from(f.code() as u8)
        }
    }
}

fn list() -> Result<(), Failure> {
    let disks = platform::list().map_err(|e| Failure::Other(format!("listing disks: {e}")))?;
    let shown: Vec<_> = disks
        .into_iter()
        .filter(|d| d.removable && !d.system && d.size_bytes > 0)
        .collect();
    let json = serde_json::to_string(&shown).map_err(|e| Failure::Other(e.to_string()))?;
    println!("{json}");
    Ok(())
}

fn lookup(id: &str, image_len: u64) -> Result<hubwriter::Disk, Failure> {
    let disks = platform::list().map_err(|e| Failure::Other(format!("listing disks: {e}")))?;
    let disk = find(&disks, id).ok_or_else(|| Failure::Refused(format!("no disk {id}")))?;
    if let Some(why) = refusal(disk, image_len) {
        return Err(Failure::Refused(why));
    }
    Ok(disk.clone())
}

fn write(image: &str, device: &str, sha: &str, progress: bool) -> Result<(), Failure> {
    let expected =
        parse_sha256(sha).ok_or_else(|| Failure::Other(format!("not a sha256: {sha}")))?;
    let image = Path::new(image);
    // refuse a wrong disk before spending a minute on the hash
    let before = lookup(device, 0)?;
    let mut stdout = io::stdout();
    let mut p = Progress::new(if progress {
        Some(&mut stdout as &mut dyn Write)
    } else {
        None
    });
    let mut image = Image::open(image)?;
    let checked = check_image(&mut image, &expected, &mut p)?;
    let disk = lookup(device, checked.len)?;
    if disk.size_bytes != before.size_bytes || disk.model != before.model {
        return Err(Failure::Refused(format!(
            "{} changed while the image was checked",
            disk.name
        )));
    }
    let mut dev = platform::open(&disk)?;
    write_image(&mut image, &checked, dev.target(), disk.sector, &mut p)?;
    dev.finish()
}
