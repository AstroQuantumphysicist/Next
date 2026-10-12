#[cfg(test)]
mod dhcp_runtime_acceptance {
    use super::*;
    use crate::{
        inject_receive, set_clock_now, take_transmit, Dma, Queue, QUEUE_BYTES, RX_FRAME, TX_FRAME,
    };

    const CLIENT_MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    const SERVER_MAC: [u8; 6] = [0x52, 0x55, 10, 0, 2, 2];
    const CLIENT_IP: [u8; 4] = [10, 0, 2, 15];
    const SERVER_IP: [u8; 4] = [10, 0, 2, 2];
    const XID: u32 = 0x4e58_5431;
    const NS: u64 = 1_000_000_000;

    fn fix_checksums(frame: &mut [u8]) {
        frame[24..26].fill(0);
        let ip_sum = udp::checksum(&frame[14..34]);
        frame[24..26].copy_from_slice(&ip_sum.to_be_bytes());
        frame[40..42].fill(0);
        let source = [frame[26], frame[27], frame[28], frame[29]];
        let destination = [frame[30], frame[31], frame[32], frame[33]];
        let sum = udp::udp_checksum(source, destination, &frame[34..]);
        frame[40..42].copy_from_slice(&(if sum == 0 { 0xffff } else { sum }).to_be_bytes());
    }

    fn dhcp_reply(
        kind: u8,
        xid: u32,
        server: [u8; 4],
        lease_seconds: u32,
        renew_after: u32,
        rebind_after: u32,
        ciaddr: [u8; 4],
        yiaddr: [u8; 4],
    ) -> Vec<u8> {
        let mut frame = vec![0; 342];
        frame[..6].copy_from_slice(&CLIENT_MAC);
        frame[6..12].copy_from_slice(if server == SERVER_IP {
            &SERVER_MAC
        } else {
            &[0x52, 0x55, 10, 0, 2, 3]
        });
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        frame[14] = 0x45;
        frame[16..18].copy_from_slice(&328u16.to_be_bytes());
        frame[20..22].copy_from_slice(&0x4000u16.to_be_bytes());
        frame[22] = 64;
        frame[23] = 17;
        frame[26..30].copy_from_slice(&server);
        frame[30..34].copy_from_slice(if kind == 6 { &[255; 4] } else { &CLIENT_IP });
        frame[34..36].copy_from_slice(&67u16.to_be_bytes());
        frame[36..38].copy_from_slice(&68u16.to_be_bytes());
        frame[38..40].copy_from_slice(&308u16.to_be_bytes());
        let p = &mut frame[42..];
        p[0] = 2;
        p[1] = 1;
        p[2] = 6;
        p[4..8].copy_from_slice(&xid.to_be_bytes());
        p[12..16].copy_from_slice(&ciaddr);
        p[16..20].copy_from_slice(&yiaddr);
        p[28..34].copy_from_slice(&CLIENT_MAC);
        p[236..240].copy_from_slice(&[99, 130, 83, 99]);
        let mut options = vec![53, 1, kind, 54, 4];
        options.extend_from_slice(&server);
        if kind != 6 {
            options.extend_from_slice(&[1, 4, 255, 255, 255, 0, 3, 4]);
            options.extend_from_slice(&[10, 0, 2, 2]);
            options.extend_from_slice(&[6, 4, 10, 0, 2, 3, 51, 4]);
            options.extend_from_slice(&lease_seconds.to_be_bytes());
            options.extend_from_slice(&[58, 4]);
            options.extend_from_slice(&renew_after.to_be_bytes());
            options.extend_from_slice(&[59, 4]);
            options.extend_from_slice(&rebind_after.to_be_bytes());
        }
        options.push(255);
        p[240..240 + options.len()].copy_from_slice(&options);
        fix_checksums(&mut frame);
        frame
    }

    fn bound_client() -> (dhcp::Client, dhcp::Lease) {
        let mut client = dhcp::Client::new(CLIENT_MAC, XID, 0).unwrap();
        let mut frame = [0; MAX_FRAME];
        client.request(0, &mut frame).unwrap();
        let offer = dhcp_reply(2, XID, SERVER_IP, 40, 20, 35, [0; 4], CLIENT_IP);
        client.accept(&offer, 1).unwrap();
        client.request(1, &mut frame).unwrap();
        let ack = dhcp_reply(5, XID, SERVER_IP, 40, 20, 35, [0; 4], CLIENT_IP);
        let lease = client.accept(&ack, 2).unwrap().unwrap();
        (client, lease)
    }

    fn queue(dma: &Dma<[u8; QUEUE_BYTES]>) -> Queue {
        Queue {
            memory: dma.0.get().cast(),
            count: 256,
            avail: 4096,
            used: 8192,
            submitted: 0,
            completed: 0,
            device_buffer: None,
        }
    }

    fn runtime_with_lease(
        client: dhcp::Client,
        lease: dhcp::Lease,
        bound_at: u64,
    ) -> (
        Runtime,
        Box<Dma<[u8; QUEUE_BYTES]>>,
        Box<Dma<[u8; QUEUE_BYTES]>>,
    ) {
        let rx_dma = Box::new(Dma(UnsafeCell::new([0; QUEUE_BYTES])));
        let tx_dma = Box::new(Dma(UnsafeCell::new([0; QUEUE_BYTES])));
        let device = Device {
            base: 0,
            mac: CLIENT_MAC,
            rx: queue(&rx_dma),
            tx: queue(&tx_dma),
            lease_at: bound_at,
            dhcp_client: Some(client),
            ipv6_router_advertisement: crate::ipv6::RouterAdvertisementState::new(),
        };
        let runtime = Runtime {
            device: Some(device),
            lease: Some(lease),
            gateway_mac: SERVER_MAC,
            expires: lease.expires_at(bound_at),
            dhcp_restarts: 0,
            tx_pending: false,
            tx_deadline: 0,
            failed: None,
        };
        (runtime, rx_dma, tx_dma)
    }

    fn request_option_codes(frame: &[u8]) -> Vec<u8> {
        let p = &frame[42..];
        let mut codes = Vec::new();
        let mut at = 240;
        while at < p.len() {
            let code = p[at];
            at += 1;
            if code == 255 {
                break;
            }
            if code == 0 {
                continue;
            }
            codes.push(code);
            at += 1 + p[at] as usize;
        }
        codes
    }

    fn pump_at(
        runtime: &mut Runtime,
        datagrams: &mut helix_core::datagram::Datagrams<8>,
        dns: &mut crate::network_socket::dns::Broker,
        now: u64,
    ) -> Result<(), NetworkError> {
        set_clock_now(now);
        runtime.run(datagrams, dns)
    }

    fn clean_host_io() {
        set_clock_now(0);
        *RX_FRAME.lock().unwrap() = None;
        TX_FRAME.lock().unwrap().clear();
    }

    #[test]
    fn retained_runtime_pump_renews_rebinds_refreshes_ack_and_recovers_after_nak() {
        clean_host_io();
        let (client, initial) = bound_client();
        let (mut runtime, _rx_dma, _tx_dma) = runtime_with_lease(client, initial, 2);
        let mut datagrams = helix_core::datagram::Datagrams::<8>::new();
        let mut dns = crate::network_socket::dns::Broker::new();
        let first_t1 = 2 + 20 * NS;

        pump_at(&mut runtime, &mut datagrams, &mut dns, first_t1).unwrap();
        let renewal = take_transmit();
        assert_eq!(&renewal[..6], &SERVER_MAC);
        assert_eq!(&renewal[26..30], &CLIENT_IP);
        assert_eq!(&renewal[30..34], &SERVER_IP);
        assert_eq!(&renewal[42 + 12..42 + 16], &CLIENT_IP);
        assert_eq!(&renewal[42 + 10..42 + 12], &[0, 0]);
        let renewal_xid = u32::from_be_bytes(renewal[46..50].try_into().unwrap());
        assert!(!request_option_codes(&renewal).contains(&50));
        assert!(!request_option_codes(&renewal).contains(&54));

        let ack_at = first_t1 + 1;
        inject_receive(&dhcp_reply(
            5,
            renewal_xid,
            SERVER_IP,
            60,
            30,
            52,
            CLIENT_IP,
            [0; 4],
        ));
        pump_at(&mut runtime, &mut datagrams, &mut dns, ack_at).unwrap();
        let renewed = runtime.lease.unwrap();
        assert_eq!(renewed.address, initial.address);
        assert_eq!(renewed.lease_seconds, 60);
        assert_eq!(runtime.expires, renewed.expires_at(ack_at));
        assert_eq!(runtime.device.as_ref().unwrap().lease_at, ack_at);
        assert_eq!(
            runtime
                .device
                .as_ref()
                .unwrap()
                .dhcp_client
                .as_ref()
                .unwrap()
                .state(),
            dhcp::State::Bound
        );

        let second_t1 = ack_at + 30 * NS;
        pump_at(&mut runtime, &mut datagrams, &mut dns, second_t1).unwrap();
        let second_renewal = take_transmit();
        assert_eq!(&second_renewal[..6], &SERVER_MAC);
        let second_xid = u32::from_be_bytes(second_renewal[46..50].try_into().unwrap());
        pump_at(&mut runtime, &mut datagrams, &mut dns, second_t1 + 1).unwrap();
        let second_t2 = ack_at + 52 * NS;
        pump_at(&mut runtime, &mut datagrams, &mut dns, second_t2).unwrap();
        let rebind = take_transmit();
        assert_eq!(&rebind[..6], &[255; 6]);
        assert_eq!(&rebind[26..30], &CLIENT_IP);
        assert_eq!(&rebind[30..34], &[255; 4]);
        assert_eq!(&rebind[42 + 12..42 + 16], &CLIENT_IP);
        assert_eq!(&rebind[42 + 10..42 + 12], &[0, 0]);
        assert_eq!(
            u32::from_be_bytes(rebind[46..50].try_into().unwrap()),
            second_xid
        );
        assert!(!request_option_codes(&rebind).contains(&50));
        assert!(!request_option_codes(&rebind).contains(&54));
        assert_eq!(
            runtime
                .device
                .as_ref()
                .unwrap()
                .dhcp_client
                .as_ref()
                .unwrap()
                .state(),
            dhcp::State::Rebinding
        );

        let rebound_at = second_t2 + 1;
        inject_receive(&dhcp_reply(
            5,
            second_xid,
            [10, 0, 2, 3],
            90,
            45,
            78,
            CLIENT_IP,
            [0; 4],
        ));
        pump_at(&mut runtime, &mut datagrams, &mut dns, rebound_at).unwrap();
        let rebound = runtime.lease.unwrap();
        assert_eq!(rebound.server, [10, 0, 2, 3]);
        assert_eq!(rebound.lease_seconds, 90);
        assert_eq!(runtime.expires, rebound.expires_at(rebound_at));

        let nak_renew_at = rebound_at + 45 * NS;
        pump_at(&mut runtime, &mut datagrams, &mut dns, nak_renew_at).unwrap();
        let nak_request = take_transmit();
        assert_eq!(&nak_request[0..6], &[0x52, 0x55, 10, 0, 2, 3]);
        let nak_xid = u32::from_be_bytes(nak_request[46..50].try_into().unwrap());
        inject_receive(&dhcp_reply(
            6,
            nak_xid,
            [10, 0, 2, 3],
            0,
            0,
            0,
            CLIENT_IP,
            [0; 4],
        ));
        pump_at(&mut runtime, &mut datagrams, &mut dns, nak_renew_at + 1).unwrap();
        assert_eq!(runtime.dhcp_restarts, 1);
        assert_eq!(
            runtime
                .device
                .as_ref()
                .unwrap()
                .dhcp_client
                .as_ref()
                .unwrap()
                .state(),
            dhcp::State::Discovering
        );
        assert_eq!(runtime.health(nak_renew_at + 1), Err(NetworkError::Dhcp));
        pump_at(&mut runtime, &mut datagrams, &mut dns, nak_renew_at + 2).unwrap();
        let recovered = take_transmit();
        assert_eq!(&recovered[..6], &[255; 6]);
        assert_eq!(&recovered[42 + 240..42 + 243], &[53, 1, 1]);
        set_clock_now(0);
    }

    #[test]
    fn retained_runtime_expires_lease_then_restarts_discovery_fail_closed() {
        clean_host_io();
        let (client, lease) = bound_client();
        let (mut runtime, _rx_dma, _tx_dma) = runtime_with_lease(client, lease, 2);
        let mut datagrams = helix_core::datagram::Datagrams::<8>::new();
        let mut dns = crate::network_socket::dns::Broker::new();
        let expires = lease.expires_at(2);
        assert_eq!(
            pump_at(&mut runtime, &mut datagrams, &mut dns, expires),
            Ok(())
        );
        assert_eq!(runtime.dhcp_restarts, 1);
        assert_eq!(runtime.health(expires), Err(NetworkError::Dhcp));
        assert_eq!(
            runtime
                .device
                .as_ref()
                .unwrap()
                .dhcp_client
                .as_ref()
                .unwrap()
                .state(),
            dhcp::State::Discovering
        );
        pump_at(&mut runtime, &mut datagrams, &mut dns, expires + 1).unwrap();
        let discovery = take_transmit();
        assert_eq!(&discovery[..6], &[255; 6]);
        assert_eq!(&discovery[42 + 240..42 + 243], &[53, 1, 1]);
        set_clock_now(0);
    }
}
