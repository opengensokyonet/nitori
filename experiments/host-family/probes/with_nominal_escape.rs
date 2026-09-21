use host_family::lending::{Root, with};

fn main() {
    let _request = with::<Root<u8>, _, _>(|access| access.into_pin());
}
