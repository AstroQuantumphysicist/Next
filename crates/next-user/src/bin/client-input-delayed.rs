#![no_std]
#![no_main]
#[path = "../client_input_fixture.rs"]
mod client_input_fixture;
next_user::entry!(probe);
fn probe(info: &next_user::StartInfo) -> ! {
    client_input_fixture::probe(info, 2000000000)
}
