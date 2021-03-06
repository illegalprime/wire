use std::net::ToSocketAddrs;
use std::net::{TcpStream, TcpListener};
use std::net::{Shutdown, SocketAddr};

use std::io::Result as IoResult;
use std::io::Error as IoError;
use std::io::{BufReader, ErrorKind};

use std::thread::spawn;

use std::marker::PhantomData;

use serialize::{Decodable, Encodable};

use bincode::{
    self,
    EncodingResult,
    EncodingError,
    DecodingError
};

use bchannel::channel;
pub use bincode::SizeLimit;
pub use bchannel::Receiver;

pub type InTcpStream<T> = Receiver<T, DecodingError>;

pub struct OutTcpStream<T> {
    tcp_stream: TcpStream,
    write_limit: SizeLimit,
    _phantom: PhantomData<T>
}

impl <'a, T> OutTcpStream<T>
where T: Encodable {
    pub fn send(&mut self, m: &T) -> EncodingResult<()> {
        bincode::encode_into(m, &mut self.tcp_stream, self.write_limit)
    }

    pub fn send_all<'b, I: Iterator<Item = &'b T>>(&mut self, mut i: I) ->
    Result<(), (&'b T, I, EncodingError)> {
        loop {
            match i.next() {
                None => return Ok(()),
                Some(x) => {
                    match self.send(x) {
                        Ok(()) => {},
                        Err(e) => return Err((x, i, e))
                    }
                }
            }
        }
    }

    pub fn close(self) {}
}

#[unsafe_destructor]
impl <T> Drop for OutTcpStream<T> {
    fn drop(&mut self) {
        self.tcp_stream.shutdown(Shutdown::Write).ok();
    }
}

/// Connect to a server and open a send-receive pair.  See `upgrade` for more
/// details.
pub fn connect_tcp<'a, 'b, I, O, A>(addr: A, read_limit: SizeLimit, write_limit: SizeLimit) ->
IoResult<(Receiver<I, DecodingError>, OutTcpStream<O>)>
where I: Send + Decodable + 'static, O: Encodable, A: ToSocketAddrs {
    Ok(try!(upgrade_tcp(try!(TcpStream::connect(&addr)), read_limit, write_limit)))
}

/// Starts listening for connections on this ip and port.
/// Returns:
/// * A receiver of Tcp stream objects.  It is recommended that you `upgrade`
///   these.
/// * A TcpAcceptor.  This can be used to close the listener from outside of the
///   listening thread.
pub fn listen_tcp<A>(addr: A) -> IoResult<(Receiver<(TcpStream, SocketAddr), IoError>, TcpListener)>
where A: ToSocketAddrs {
    let tcpl = try!(TcpListener::bind(&addr));
    let (sx, rx) = channel();

    let tcpl2 = try!(tcpl.try_clone());
    spawn(move || {
        loop {
            if sx.is_closed() {
                break;
            }
            match tcpl2.accept() {
                Ok(stream) => {
                    if sx.send(stream).is_err() {
                        break;
                    }
                }
                Err(ref e) if e.kind() == ErrorKind::TimedOut => {
                    continue;
                }
                Err(e) => {
                    let _  = sx.error(e);
                    break;
                }
            }
        }
    });
    Ok((rx, tcpl))
}

/// Upgrades a TcpStream to a Sender-Receiver pair that you can use to send and
/// receive objects automatically.  If there is an error decoding or encoding
/// values, that respective part is shut down.
pub fn upgrade_tcp<'a, 'b, I, O>(
    stream: TcpStream, read_limit: SizeLimit, write_limit: SizeLimit) ->
IoResult<(InTcpStream<I>, OutTcpStream<O>)>
where I: Send + Decodable + 'static, O: Encodable {
    let s1 = stream;
    let s2 = try!(s1.try_clone());
    Ok((upgrade_reader(s1, read_limit),
     upgrade_writer(s2, write_limit)))
}

fn upgrade_writer<'a, T>(stream: TcpStream, write_limit: SizeLimit) -> OutTcpStream<T>
where T: Encodable {
    OutTcpStream {
        tcp_stream: stream,
        write_limit: write_limit,
        _phantom: PhantomData
    }
}

fn upgrade_reader<'a, T>(stream: TcpStream, read_limit: SizeLimit) -> InTcpStream<T>
where T: Send + Decodable + 'static {
    let (in_snd, in_rec) = channel();

    spawn(move || {
        let mut buffer = BufReader::new(stream);
        let read_limit = read_limit;
        loop {
            match bincode::decode_from(&mut buffer, read_limit) {
                Ok(a) => {
                    // Try to send, and if we can't, then the channel is closed.
                    if in_snd.send(a).is_err() {
                        break;
                    }
                },
                // if we can't decode, close the stream with an error.
                Err(e) => {
                    let _ = in_snd.error(e);
                    break;
                }
            }
        }
        let s1 = buffer.into_inner();
        let _ = s1.shutdown(Shutdown::Read);
    });
    in_rec
}
