#![no_std]
#![no_main]
use next_user::{
    self as rt,
    sdk::{Datagram, Rights, Wait},
};
rt::entry!(blocked);

fn blocked(info: &rt::StartInfo) -> ! {
    let socket = Datagram::create(49207, Rights::READ.union(Rights::WRITE).union(Rights::WAIT))
        .unwrap_or_else(|_| rt::exit(240));
    let mut packet = [0; 24];
    packet[..8].copy_from_slice(b"NEXTWAIT");
    packet[8..16].copy_from_slice(&2u64.to_le_bytes());
    packet[16..].copy_from_slice(&info.pid.to_le_bytes());
    if socket.send(&packet) != Ok(24) {
        rt::exit(241);
    }
    rt::log(
        info.log,
        "UDP BLOCK: connected port 49207 acquired; parking readable for kill/reopen proof\n",
    );
    // A console-owned process is terminated by the trusted console. Its socket
    // and registration must disappear together, leaving this exact port reusable.
    let _ = socket.wait_readable(Wait::Block);
    rt::exit(242)
}
