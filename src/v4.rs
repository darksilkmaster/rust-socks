use byteorder::{ReadBytesExt, WriteBytesExt, BigEndian};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs, SocketAddrV4, SocketAddrV6, TcpStream, Ipv4Addr};

use {ToTargetAddr, TargetAddr};

fn read_response(socket: &mut TcpStream) -> io::Result<SocketAddrV4> {
    let mut response = [0u8; 8];
    try!(socket.read_exact(&mut response));
    let mut response = &response[..];

    if try!(response.read_u8()) != 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid response version"));
    }

    match try!(response.read_u8()) {
        90 => {}
        91 => return Err(io::Error::new(io::ErrorKind::Other, "request rejected or failed")),
        92 => {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                                      "request rejected because SOCKS server cannot connect to \
                                       idnetd on the client"))
        }
        93 => {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                                      "request rejected because the client program and identd \
                                       report different user-ids"))
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid response code")),
    }

    let port = try!(response.read_u16::<BigEndian>());
    let ip = Ipv4Addr::from(try!(response.read_u32::<BigEndian>()));

    Ok(SocketAddrV4::new(ip, port))
}

/// A SOCKS4 client.
#[derive(Debug)]
pub struct Socks4Stream {
    socket: TcpStream,
    proxy_addr: SocketAddrV4,
}

impl Socks4Stream {
    /// Connects to a target server through a SOCKS4 proxy.
    ///
    /// # Note
    ///
    /// If `target` is a `TargetAddr::Domain`, the domain name will be forwarded
    /// to the proxy server using the SOCKS4A protocol extension. If the proxy
    /// server does not support SOCKS4A, consider performing the DNS lookup
    /// locally and passing a `TargetAddr::Ip`.
    pub fn connect<T, U>(proxy: T, target: U, userid: &str) -> io::Result<Socks4Stream>
        where T: ToSocketAddrs,
              U: ToTargetAddr
    {
        Self::connect_raw(1, proxy, target, userid)
    }

    fn connect_raw<T, U>(command: u8, proxy: T, target: U, userid: &str) -> io::Result<Socks4Stream>
        where T: ToSocketAddrs,
              U: ToTargetAddr
    {
        let mut socket = try!(TcpStream::connect(proxy));

        let target = try!(target.to_target_addr());

        let mut packet = vec![];
        let _ = packet.write_u8(4); // version
        let _ = packet.write_u8(command); // command code
        match try!(target.to_target_addr()) {
            TargetAddr::Ip(addr) => {
                let addr = match addr {
                    SocketAddr::V4(addr) => addr,
                    SocketAddr::V6(_) => {
                        return Err(io::Error::new(io::ErrorKind::InvalidInput,
                                                  "SOCKS4 does not support IPv6"));
                    }
                };
                let _ = packet.write_u16::<BigEndian>(addr.port());
                let _ = packet.write_u32::<BigEndian>((*addr.ip()).into());
                let _ = packet.write_all(userid.as_bytes());
                let _ = packet.write_u8(0);
            }
            TargetAddr::Domain(ref host, port) => {
                let _ = packet.write_u16::<BigEndian>(port);
                let _ = packet.write_u32::<BigEndian>(Ipv4Addr::new(0, 0, 0, 1).into());
                let _ = packet.write_all(userid.as_bytes());
                let _ = packet.write_u8(0);
                let _ = packet.extend(host.as_bytes());
                let _ = packet.write_u8(0);
            }
        }

        try!(socket.write_all(&packet));
        let proxy_addr = try!(read_response(&mut socket));

        Ok(Socks4Stream {
            socket: socket,
            proxy_addr: proxy_addr,
        })
    }

    /// Returns the proxy-side address of the connection between the proxy and
    /// target server.
    pub fn proxy_addr(&self) -> SocketAddrV4 {
        self.proxy_addr
    }

    /// Returns a shared reference to the inner `TcpStream`.
    pub fn get_ref(&self) -> &TcpStream {
        &self.socket
    }

    /// Returns a mutable reference to the inner `TcpStream`.
    pub fn get_mut(&mut self) -> &mut TcpStream {
        &mut self.socket
    }

    /// Consumes the `Socks4Stream`, returning the inner `TcpStream`.
    pub fn into_inner(self) -> TcpStream {
        self.socket
    }
}

impl Read for Socks4Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.socket.read(buf)
    }
}

impl<'a> Read for &'a Socks4Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.socket).read(buf)
    }
}

impl Write for Socks4Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.socket.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}

impl<'a> Write for &'a Socks4Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.socket).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&self.socket).flush()
    }
}

#[derive(Debug)]
pub struct Socks4Listener(Socks4Stream);

impl Socks4Listener {
    pub fn bind<T, U>(proxy: T, target: U, userid: &str) -> io::Result<Socks4Listener>
        where T: ToSocketAddrs,
              U: ToTargetAddr
    {
        Socks4Stream::connect_raw(2, proxy, target, userid).map(Socks4Listener)
    }

    pub fn proxy_addr(&self) -> io::Result<SocketAddr> {
        if self.0.proxy_addr.ip().octets() != [0, 0, 0, 0] {
            Ok(SocketAddr::V4(self.0.proxy_addr()))
        } else {
            let port = self.0.proxy_addr.port();
            let peer = match try!(self.0.socket.peer_addr()) {
                SocketAddr::V4(addr) => SocketAddr::V4(SocketAddrV4::new(*addr.ip(), port)),
                SocketAddr::V6(addr) => SocketAddr::V6(SocketAddrV6::new(*addr.ip(), port, 0, 0)),
            };
            Ok(peer)
        }
    }

    pub fn accept(mut self) -> io::Result<Socks4Stream> {
        self.0.proxy_addr = try!(read_response(&mut self.0.socket));
        Ok(self.0)
    }
}