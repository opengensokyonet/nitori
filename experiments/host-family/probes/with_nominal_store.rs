use host_family::lending::{Root, with};

fn main() {
    let mut saved = None;
    let _request = with::<Root<u8>, _, _>(|access| {
        saved = Some(access.into_pin());
    });
}
