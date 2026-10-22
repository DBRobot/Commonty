//! The fleet's names as every client defaults to them, in one place.
//! checks.fleet-defaults holds them to fleet/policy.json, fleet/boxes.json
//! and fleet/release.pub.

macro_rules! domain {
    () => {
        "commonty.org"
    };
}
macro_rules! repo {
    () => {
        "david/commonty"
    };
}
macro_rules! cache_box {
    () => {
        "100.64.0.2"
    };
}
macro_rules! cache_box_name {
    () => {
        "node1"
    };
}

pub const DOMAIN: &str = domain!();
/// the front door's directory: the one name that answers from anywhere
pub const DIRECTORY: &str = concat!("https://home.", domain!(), "/_dd/directory");
pub const ENTE: &str = concat!("https://api.", domain!());
pub const IMAGES: &str = concat!("https://files.", domain!(), "/images/");
/// any browser-facing host does; the session cookie covers the whole domain
pub const ENROL: &str = concat!("https://home.", domain!(), "/_dd/enrol");
/// the release key's public half (fleet/release.pub): what a client checks
/// the member list against, the one thing no box can make
pub const RELEASE_KEY: &str = "fdpYPlUGQCA+lolOtDKO1cgc4bM1syI3JX/0hFPthvo=";
/// The oldest member list a client takes: the release counter of the last
/// one that shut someone out (or 0, before any did). A list is only ever
/// signed, never re-signed, so without this a device's first look would
/// take any list ever published. `dd release publish` refuses a release
/// that revokes someone unless this is at least its counter, so every
/// client built from it holds out for that list or a newer one.
pub const MEMBER_LIST_FLOOR: u64 = 252;
/// where boxes read the release from
pub const RELEASES: &str = concat!(
    "https://git.",
    domain!(),
    "/",
    repo!(),
    "/raw/branch/releases/current.json"
);
/// the nix-cache bucket, in the store of its home (node1: the first store
/// able to run the cache, nix/lib/fleet-placement.nix homes), at its name on
/// the fleet's network (nix/modules/storage/store.nix shared)
pub const CACHE: &str = concat!(
    "s3://nix-cache?endpoint=store.",
    cache_box_name!(),
    ".",
    domain!(),
    ":3443&scheme=https&region=us-east-1"
);
/// that box over ssh, where its store's key to the cache is read
pub const CACHE_BOX: &str = cache_box!();
