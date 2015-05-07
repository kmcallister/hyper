//! A collection of traits abstracting over Listeners and Streams.
use std::any::{Any, TypeId};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, ToSocketAddrs, TcpStream, TcpListener, Shutdown};
use std::mem;
use std::path::Path;
use std::sync::Arc;

#[cfg(feature = "suruga-https")] use std::sync::Mutex;
#[cfg(feature = "suruga-https")] use rand;
#[cfg(feature = "suruga-https")] use suruga;

use openssl::ssl::{SslContext, SSL_VERIFY_NONE};
use openssl::ssl::SslMethod::Sslv23;
use openssl::x509::X509FileType;

#[cfg(not(feature = "suruga-https"))]
use openssl::ssl::{Ssl, SslStream};

#[cfg(not(feature = "suruga-https"))]
use openssl::ssl::error::StreamError as SslIoError;

use typeable::Typeable;
use {traitobject};

/// The write-status indicating headers have not been written.
pub enum Fresh {}

/// The write-status indicating headers have been written.
pub enum Streaming {}

/// An abstraction to listen for connections on a certain port.
pub trait NetworkListener: Clone {
    /// The stream produced for each connection.
    type Stream: NetworkStream + Send + Clone;
    /// Listens on a socket.
    //fn listen<To: ToSocketAddrs>(&mut self, addr: To) -> io::Result<Self::Acceptor>;

    /// Returns an iterator of streams.
    fn accept(&mut self) -> ::Result<Self::Stream>;

    /// Get the address this Listener ended up listening on.
    fn local_addr(&mut self) -> io::Result<SocketAddr>;

    /// Closes the Acceptor, so no more incoming connections will be handled.
//    fn close(&mut self) -> io::Result<()>;

    /// Returns an iterator over incoming connections.
    fn incoming(&mut self) -> NetworkConnections<Self> {
        NetworkConnections(self)
    }
}

/// An iterator wrapper over a NetworkAcceptor.
pub struct NetworkConnections<'a, N: NetworkListener + 'a>(&'a mut N);

impl<'a, N: NetworkListener + 'a> Iterator for NetworkConnections<'a, N> {
    type Item = ::Result<N::Stream>;
    fn next(&mut self) -> Option<::Result<N::Stream>> {
        Some(self.0.accept())
    }
}

/// An abstraction over streams that a Server can utilize.
pub trait NetworkStream: Read + Write + Any + Send + Typeable {
    /// Get the remote address of the underlying connection.
    fn peer_addr(&mut self) -> io::Result<SocketAddr>;
    /// This will be called when Stream should no longer be kept alive.
    #[inline]
    fn close(&mut self, _how: Shutdown) -> io::Result<()> {
        Ok(())
    }
}

/// A connector creates a NetworkStream.
pub trait NetworkConnector {
    /// Type of Stream to create
    type Stream: Into<Box<NetworkStream + Send>>;
    /// Connect to a remote address.
    fn connect(&mut self, host: &str, port: u16, scheme: &str) -> ::Result<Self::Stream>;
}

impl<T: NetworkStream + Send> From<T> for Box<NetworkStream + Send> {
    fn from(s: T) -> Box<NetworkStream + Send> {
        Box::new(s)
    }
}

impl fmt::Debug for Box<NetworkStream + Send> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.pad("Box<NetworkStream>")
    }
}

impl NetworkStream + Send {
    unsafe fn downcast_ref_unchecked<T: 'static>(&self) -> &T {
        mem::transmute(traitobject::data(self))
    }

    unsafe fn downcast_mut_unchecked<T: 'static>(&mut self) -> &mut T {
        mem::transmute(traitobject::data_mut(self))
    }

    unsafe fn downcast_unchecked<T: 'static>(self: Box<NetworkStream + Send>) -> Box<T>  {
        let raw: *mut NetworkStream = mem::transmute(self);
        mem::transmute(traitobject::data_mut(raw))
    }
}

impl NetworkStream + Send {
    /// Is the underlying type in this trait object a T?
    #[inline]
    pub fn is<T: Any>(&self) -> bool {
        (*self).get_type() == TypeId::of::<T>()
    }

    /// If the underlying type is T, get a reference to the contained data.
    #[inline]
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { self.downcast_ref_unchecked() })
        } else {
            None
        }
    }

    /// If the underlying type is T, get a mutable reference to the contained
    /// data.
    #[inline]
    pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe { self.downcast_mut_unchecked() })
        } else {
            None
        }
    }

    /// If the underlying type is T, extract it.
    #[inline]
    pub fn downcast<T: Any>(self: Box<NetworkStream + Send>)
            -> Result<Box<T>, Box<NetworkStream + Send>> {
        if self.is::<T>() {
            Ok(unsafe { self.downcast_unchecked() })
        } else {
            Err(self)
        }
    }
}

/// A `NetworkListener` for `HttpStream`s.
pub enum HttpListener {
    /// Http variant.
    Http(TcpListener),
    /// Https variant. The two paths point to the certificate and key PEM files, in that order.
    Https(TcpListener, Arc<SslContext>)
}

impl Clone for HttpListener {
    fn clone(&self) -> HttpListener {
        match *self {
            HttpListener::Http(ref tcp) => HttpListener::Http(tcp.try_clone().unwrap()),
            HttpListener::Https(ref tcp, ref ssl) => HttpListener::Https(tcp.try_clone().unwrap(), ssl.clone()),
        }
    }
}

impl HttpListener {

    /// Start listening to an address over HTTP.
    pub fn http<To: ToSocketAddrs>(addr: To) -> ::Result<HttpListener> {
        Ok(HttpListener::Http(try!(TcpListener::bind(addr))))
    }

    /// Start listening to an address over HTTPS.
    pub fn https<To: ToSocketAddrs>(addr: To, cert: &Path, key: &Path) -> ::Result<HttpListener> {
        let mut ssl_context = try!(SslContext::new(Sslv23));
        try!(ssl_context.set_cipher_list("DEFAULT"));
        try!(ssl_context.set_certificate_file(cert, X509FileType::PEM));
        try!(ssl_context.set_private_key_file(key, X509FileType::PEM));
        ssl_context.set_verify(SSL_VERIFY_NONE, None);
        HttpListener::https_with_context(addr, ssl_context)
    }

    /// Start listening to an address of HTTPS using the given SslContext
    pub fn https_with_context<To: ToSocketAddrs>(addr: To, ssl_context: SslContext) -> ::Result<HttpListener> {
        Ok(HttpListener::Https(try!(TcpListener::bind(addr)), Arc::new(ssl_context)))
    }
}

impl NetworkListener for HttpListener {
    type Stream = HttpStream;

    #[inline]
    fn accept(&mut self) -> ::Result<HttpStream> {
        match *self {
            HttpListener::Http(ref mut tcp) => Ok(HttpStream::Http(CloneTcpStream(try!(tcp.accept()).0))),

            #[cfg(not(feature = "suruga-https"))]
            HttpListener::Https(ref mut tcp, ref ssl_context) => {
                let stream = CloneTcpStream(try!(tcp.accept()).0);
                match SslStream::new_server(&**ssl_context, stream) {
                    Ok(ssl_stream) => Ok(HttpStream::Https(ssl_stream)),
                    Err(SslIoError(e)) => {
                        Err(io::Error::new(io::ErrorKind::ConnectionAborted, e).into())
                    },
                    Err(e) => Err(e.into())
                }
            }

            #[cfg(feature = "suruga-https")]
            HttpListener::Https(..) => panic!("hyper HTTPS server not supported with suruga"),
        }
    }

    #[inline]
    fn local_addr(&mut self) -> io::Result<SocketAddr> {
        match *self {
            HttpListener::Http(ref mut tcp) => tcp.local_addr(),
            HttpListener::Https(ref mut tcp, _) => tcp.local_addr(),
        }
    }
}

#[doc(hidden)]
pub struct CloneTcpStream(TcpStream);

impl Clone for CloneTcpStream{
    #[inline]
    fn clone(&self) -> CloneTcpStream {
        CloneTcpStream(self.0.try_clone().unwrap())
    }
}

impl Read for CloneTcpStream {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for CloneTcpStream {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }
    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// HTTP over SSL, as implemented by
/// [suruga](https://github.com/klutzy/suruga).
///
/// This is EXPERIMENTAL and INSECURE! There is currently
/// no facility for certificate verification.
#[cfg(feature = "suruga-https")]
pub type HttpsStream = Arc<Mutex<suruga::TlsClient<CloneTcpStream, CloneTcpStream>>>;

/// Stream for HTTPS using OpenSSL.
#[cfg(not(feature = "suruga-https"))]
pub type HttpsStream = SslStream<CloneTcpStream>;

#[cfg(feature = "suruga-https")]
macro_rules! https_stream {
    ($e:expr) => { $e.lock().unwrap() }
}

#[cfg(not(feature = "suruga-https"))]
macro_rules! https_stream {
    ($e:expr) => { $e }
}

/// A wrapper around a TcpStream.
#[derive(Clone)]
pub enum HttpStream {
    /// A stream over the HTTP protocol.
    Http(CloneTcpStream),
    /// A stream over the HTTP protocol, protected by SSL.
    Https(HttpsStream),
}

impl fmt::Debug for HttpStream {
  fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
    match *self {
      HttpStream::Http(_) => write!(fmt, "Http HttpStream"),
      HttpStream::Https(_) => write!(fmt, "Https HttpStream"),
    }
  }
}

impl Read for HttpStream {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            HttpStream::Http(ref mut inner) => inner.read(buf),
            HttpStream::Https(ref mut inner) => https_stream!(inner).read(buf),
        }
    }
}

impl Write for HttpStream {
    #[inline]
    fn write(&mut self, msg: &[u8]) -> io::Result<usize> {
        match *self {
            HttpStream::Http(ref mut inner) => inner.write(msg),
            HttpStream::Https(ref mut inner) => https_stream!(inner).write(msg),
        }
    }
    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        match *self {
            HttpStream::Http(ref mut inner) => inner.flush(),
            HttpStream::Https(ref mut inner) => https_stream!(inner).flush(),
        }
    }
}

impl NetworkStream for HttpStream {
    #[inline]
    fn peer_addr(&mut self) -> io::Result<SocketAddr> {
        match *self {
            HttpStream::Http(ref mut inner) => inner.0.peer_addr(),
            #[cfg(not(feature = "suruga-https"))]
            HttpStream::Https(ref mut inner) => inner.get_mut().0.peer_addr(),
            #[cfg(feature = "suruga-https")]
            HttpStream::Https(ref mut inner) => inner.lock().unwrap().reader().0.peer_addr(),
        }
    }

    #[inline]
    fn close(&mut self, how: Shutdown) -> io::Result<()> {
        match *self {
            HttpStream::Http(ref mut inner) => inner.0.shutdown(how),
            #[cfg(not(feature = "suruga-https"))]
            HttpStream::Https(ref mut inner) => inner.get_mut().0.shutdown(how),
            #[cfg(feature = "suruga-https")]
            HttpStream::Https(ref mut inner) => inner.lock().unwrap().reader().0.shutdown(how),
        }
    }
}

/// A connector that will produce HttpStreams.
pub struct HttpConnector(pub Option<ContextVerifier>);

/// A method that can set verification methods on an SSL context
pub type ContextVerifier = Box<FnMut(&mut SslContext) -> () + Send>;

impl NetworkConnector for HttpConnector {
    type Stream = HttpStream;

    fn connect(&mut self, host: &str, port: u16, scheme: &str) -> ::Result<HttpStream> {
        let addr = &(host, port);
        Ok(try!(match scheme {
            "http" => {
                debug!("http scheme");
                Ok(HttpStream::Http(CloneTcpStream(try!(TcpStream::connect(addr)))))
            },
            #[cfg(not(feature = "suruga-https"))]
            "https" => {
                debug!("https scheme");
                let stream = CloneTcpStream(try!(TcpStream::connect(addr)));
                let mut context = try!(SslContext::new(Sslv23));
                if let Some(ref mut verifier) = self.0 {
                    verifier(&mut context);
                }
                let ssl = try!(Ssl::new(&context));
                try!(ssl.set_hostname(host));
                let stream = try!(SslStream::new(&context, stream));
                Ok(HttpStream::Https(stream))
            },
            #[cfg(feature = "suruga-https")]
            "https" => {
                debug!("https scheme");
                let stream = CloneTcpStream(try!(TcpStream::connect(addr)));
                Ok(HttpStream::Https(Arc::new(Mutex::new({
                    let rng = rand::OsRng::new().unwrap();
                    suruga::TlsClient::new(stream.clone(), stream, rng).unwrap()
                }))))
            }
            _ => {
                Err(io::Error::new(io::ErrorKind::InvalidInput,
                                "Invalid scheme for Http"))
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use mock::MockStream;
    use super::NetworkStream;

    #[test]
    fn test_downcast_box_stream() {
        // FIXME: Use Type ascription
        let stream: Box<NetworkStream + Send> = Box::new(MockStream::new());

        let mock = stream.downcast::<MockStream>().ok().unwrap();
        assert_eq!(mock, Box::new(MockStream::new()));

    }

    #[test]
    fn test_downcast_unchecked_box_stream() {
        // FIXME: Use Type ascription
        let stream: Box<NetworkStream + Send> = Box::new(MockStream::new());

        let mock = unsafe { stream.downcast_unchecked::<MockStream>() };
        assert_eq!(mock, Box::new(MockStream::new()));

    }

}
