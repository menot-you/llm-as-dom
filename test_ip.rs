use std::net::IpAddr;

fn main() {
    let ip: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
    
    println!("Parsed IP: {}", ip);
    println!("Is V4? {}", ip.is_ipv4());
    println!("Is V6? {}", ip.is_ipv6());
    println!("is_loopback? {}", ip.is_loopback());
    
    // We match to get the specific Ipv6Addr behavior since is_private and is_link_local
    // aren't fully stabilized directly on IpAddr, but we can check the stable methods available.
    if let IpAddr::V6(v6) = ip {
        println!("is_unicast_link_local? {}", v6.is_unicast_link_local());
        
        // to_ipv4_mapped() returns Some(Ipv4Addr) if it's an IPv4-mapped IPv6 address
        if let Some(v4) = v6.to_ipv4_mapped() {
            println!("Extracted IPv4 mapped: {}", v4);
            println!("Mapped IPv4 is_loopback? {}", v4.is_loopback());
            println!("Mapped IPv4 is_link_local? {}", v4.is_link_local_addr());
            // v4.is_private() is still unstable, but 127.0.0.1 is not private, it's loopback.
        }
    }
}
