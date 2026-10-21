//! Against a real museum and store (nix/tests/photos.nix): an account made
//! with the fleet's code, a file saved as Mail saves one, read back whole
//! with its small picture, and a link made to it. Exits non-zero on the
//! first thing that is not so.
//!
//! ente-roundtrip <museum> <email> <password> <code> [picture]
//!
//! With a picture (a JPEG), it is saved too, as itself and its own small
//! picture: something a gallery can show (nix/tests/site-journeys.nix).

use anyhow::{Context, Result, anyhow, ensure};
use ente::gallery::{Gallery, IMAGE, NoUi};
use ente::{AuthFlow, CreateAccountParams};
use zeroize::Zeroizing;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    let picture = if a.len() == 5 { a.pop() } else { None };
    let [api, email, password, code] = <[String; 4]>::try_from(a)
        .map_err(|_| anyhow!("ente-roundtrip <museum> <email> <password> <code> [picture]"))?;
    let client = ente::client(&api)?;
    client.send_otp(&email, "signup").await?;
    AuthFlow::new(&client, &mut NoUi)
        .create_account_with_otp(
            CreateAccountParams {
                email: email.clone(),
                password: Zeroizing::new(password.clone()),
                source: None,
            },
            &code,
        )
        .await
        .context("making the account")?;
    let g = Gallery::sign_in(&api, &email, &password).await?;

    // past one 4 MiB message, so the file goes as Ente's apps send a big one
    let data: Vec<u8> = (0..(5 << 20) + 123).map(|i| (i * 7 % 251) as u8).collect();
    let thumb: Vec<u8> = (0..2048).map(|i| (i % 13) as u8).collect();
    let taken = 1_760_000_000_000_000i64;
    let id = g
        .save("From Mail", "beach.jpg", IMAGE, taken, &data, &thumb)
        .await
        .context("saving")?;
    // a second into the same album, which is found, not made again
    let id2 = g
        .save("From Mail", "two.jpg", IMAGE, taken + 1, b"second", &thumb)
        .await
        .context("saving a second")?;
    println!("saved {id} and {id2}");

    let (albums, photos) = g.photos().await?;
    let from_mail: Vec<_> = albums.iter().filter(|a| a.name == "From Mail").collect();
    ensure!(
        from_mail.len() == 1,
        "one album From Mail, not {}",
        from_mail.len()
    );
    let p = photos
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("the saved file is not listed"))?;
    ensure!(
        p.title == "beach.jpg",
        "its title came back as {:?}",
        p.title
    );
    ensure!(p.taken == taken, "its time came back as {}", p.taken);
    ensure!(p.albums == vec![from_mail[0].id], "it is in {:?}", p.albums);
    ensure!(g.thumbnail(p).await? == thumb, "its small picture differs");
    ensure!(g.original(p, 64 << 20).await? == data, "the file differs");
    println!("read back whole");

    let link = g
        .share("Shared from Mail", &[p.clone()])
        .await
        .context("sharing")?;
    let (at, key) = link
        .split_once('#')
        .ok_or_else(|| anyhow!("no key in the link: {link}"))?;
    ensure!(at.starts_with("https://albums."), "the link is at {at}");
    ensure!(key.len() >= 40, "the key in the link is {key}");
    println!("link {at}#...");
    if let Some(path) = picture {
        let jpeg = std::fs::read(&path).with_context(|| format!("reading {path}"))?;
        let id = g
            .save("Otters", "otter.jpg", IMAGE, taken + 2, &jpeg, &jpeg)
            .await
            .context("saving the picture")?;
        println!("saved the picture as {id}");
    }
    println!("roundtrip ok");
    Ok(())
}
