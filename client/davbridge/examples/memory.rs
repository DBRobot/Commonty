//! The bridge over memory, for trying a calendar program against it:
//! `cargo run -p davbridge --example memory -- 41699 <password>`.

use std::sync::Arc;

use davbridge::{Kind, Memory, router};

#[tokio::main]
async fn main() {
    let mut a = std::env::args().skip(1);
    let port: u16 = a.next().and_then(|p| p.parse().ok()).unwrap_or(41699);
    let pw = a.next().unwrap_or_else(|| "try".into());
    let m = Memory::with(Kind::Calendars, "personal", "Personal", true);
    m.add(Kind::Contacts, "contacts", "Contacts", true);
    let l = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    axum::serve(l, router(Arc::new(m), pw, port)).await.unwrap();
}
