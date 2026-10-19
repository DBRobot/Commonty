//! Photos from another of the member's pages (Mail): the account opened in
//! the tab with the password the app hands it (/_dd/app/keys), then its
//! photos listed, opened, saved to and shared, as Ente's own apps do it.
//! Museum and the store are asked by way of the page's own origin, which
//! the app passes on (app/src/keys.rs): the page may ask nothing else.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ente::gallery::{Gallery, Photo};
use wasm_bindgen::prelude::*;

struct Open {
    gallery: Gallery,
    photos: RefCell<Vec<Photo>>,
}

thread_local! {
    static OPEN: RefCell<HashMap<u32, Rc<Open>>> = RefCell::new(HashMap::new());
}

fn js(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

fn get(h: u32) -> Result<Rc<Open>, JsValue> {
    OPEN.with(|o| o.borrow().get(&h).cloned())
        .ok_or_else(|| js("Photos is not open in this tab"))
}

fn photo(o: &Open, id: f64) -> Result<Photo, JsValue> {
    o.photos
        .borrow()
        .iter()
        .find(|p| p.id as f64 == id)
        .cloned()
        .ok_or_else(|| js("no such photo"))
}

/// Sign in; `api` and `store` are the page's own addresses that lead to
/// museum and the store. A handle comes back for the calls below.
#[wasm_bindgen]
pub async fn photos_open(
    api: &str,
    store: &str,
    email: &str,
    password: &str,
) -> Result<u32, JsValue> {
    let gallery = Gallery::sign_in_with(api, api, email, password, reqwest::Client::new())
        .await
        .map_err(js)?
        .with_store_via(store);
    let h = OPEN.with(|o| {
        let mut o = o.borrow_mut();
        let h = o.keys().max().map_or(1, |m| m + 1);
        o.insert(
            h,
            Rc::new(Open {
                gallery,
                photos: RefCell::new(Vec::new()),
            }),
        );
        h
    });
    Ok(h)
}

/// Every photo, newest first, and the albums they are in, as json:
/// `{albums:[{id,name}], photos:[{id,title,taken,kind,albums}]}`; `taken`
/// in milliseconds
#[wasm_bindgen]
pub async fn photos_list(h: u32) -> Result<String, JsValue> {
    let o = get(h)?;
    let (albums, photos) = o.gallery.photos().await.map_err(js)?;
    let v = serde_json::json!({
        "albums": albums.iter().map(|a| serde_json::json!({ "id": a.id, "name": a.name, "kind": a.kind })).collect::<Vec<_>>(),
        "photos": photos.iter().map(|p| serde_json::json!({
            "id": p.id,
            "title": p.title,
            "taken": p.taken / 1000,
            "kind": p.kind,
            "albums": p.albums,
        })).collect::<Vec<_>>(),
    });
    *o.photos.borrow_mut() = photos;
    Ok(v.to_string())
}

#[wasm_bindgen]
pub async fn photos_thumbnail(h: u32, id: f64) -> Result<Vec<u8>, JsValue> {
    let o = get(h)?;
    let p = photo(&o, id)?;
    o.gallery.thumbnail(&p).await.map_err(js)
}

/// the file itself, whatever its kind, up to `cap` bytes
#[wasm_bindgen]
pub async fn photos_original(h: u32, id: f64, cap: f64) -> Result<Vec<u8>, JsValue> {
    let o = get(h)?;
    let p = photo(&o, id)?;
    let mut s = o.gallery.original_stream(&p).await.map_err(js)?;
    let mut out = Vec::new();
    while let Some(b) = s.next().await.map_err(js)? {
        out.extend_from_slice(&b);
        if out.len() as f64 > cap {
            return Err(js("too big to attach"));
        }
    }
    Ok(out)
}

/// Save a file into the album `album`; `kind` 0 an image, 1 a video;
/// `taken` in milliseconds; `thumbnail` a small JPEG the page drew. The
/// new photo's id comes back.
#[wasm_bindgen]
pub async fn photos_save(
    h: u32,
    album: &str,
    title: &str,
    kind: f64,
    taken: f64,
    data: Vec<u8>,
    thumbnail: Vec<u8>,
) -> Result<f64, JsValue> {
    let o = get(h)?;
    o.gallery
        .save(
            album,
            title,
            kind as i64,
            (taken as i64) * 1000,
            &data,
            &thumbnail,
        )
        .await
        .map(|id| id as f64)
        .map_err(js)
}

/// A link anyone can open these photos at (ids from `photos_list`)
#[wasm_bindgen]
pub async fn photos_share(h: u32, name: &str, ids: Vec<f64>) -> Result<String, JsValue> {
    let o = get(h)?;
    let photos = ids
        .iter()
        .map(|&id| photo(&o, id))
        .collect::<Result<Vec<_>, _>>()?;
    o.gallery.share(name, &photos).await.map_err(js)
}

#[wasm_bindgen]
pub fn photos_close(h: u32) {
    OPEN.with(|o| o.borrow_mut().remove(&h));
}
