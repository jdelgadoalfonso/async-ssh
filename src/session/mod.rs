use tokio::io::{AsyncRead, AsyncWrite};
use std::sync::{Arc, Mutex};
use futures::Future;
use channel;
use {Connection, SharableConnection};
use thrussh;
use thrussh_keys;

pub(crate) mod state;

/// A newly established, unauthenticated SSH session.
///
/// All you can really do with this in authenticate it using one of the `authenticate_*` methods.
/// You'll most likely want [`NewSession::authenticate_key`].
pub struct NewSession<S: AsyncRead + AsyncWrite + Send> {
    c: Connection<S>,
}

impl<S: AsyncRead + AsyncWrite + Send + 'static> NewSession<S> {
    /// Authenticate as the given user using the given keypair.
    ///
    /// See also
    /// [`thrussh::client::Connection::authenticate_key`](https://docs.rs/thrussh/0.21/thrussh/client/struct.Connection.html#method.authenticate_key).
    pub fn authenticate_key(
        self,
        user: &str,
        key: thrussh_keys::key::KeyPair,
    ) -> Box<dyn Future<Item = Session<S>, Error = thrussh::HandlerError<()>> + Send>
    where
        S: thrussh::Tcp,
    {
        let NewSession { c } = self;
        Box::new(
            c.c
                .authenticate_key(user, Arc::new(key))
                .map(move |c| Session::make(Connection { c, task: None })),
        )
    }

    /// Authenticate as the given user using a plain password.
    ///
    /// See also
    /// [`thrussh::client::Connection::authenticate_password`](https://docs.rs/thrussh/0.21/thrussh/client/struct.Connection.html#method.authenticate_password).
    pub fn authenticate_password(
        self,
        user: &str,
        pass: String,
    ) -> Box<dyn Future<Item = Session<S>, Error = thrussh::HandlerError<()>> + Send>
    where
        S: thrussh::Tcp,
    {
        let NewSession { c } = self;
        Box::new(
            c.c
                .authenticate_password(user, pass)
                .map(move |c| Session::make(Connection { c, task: None })),
        )
    }
}

/// An established and authenticated SSH session.
///
/// You can use this session to execute commands on the remote host using [`Session::open_exec`].
/// This will give you back a [`Channel`], which can be used to read from the resulting process'
/// `STDOUT`, or to write the the process' `STDIN`.
pub struct Session<S: AsyncRead + AsyncWrite + Send>(SharableConnection<S>);

impl<S: AsyncRead + AsyncWrite + thrussh::Tcp + Send + 'static> Session<S> {
    /// Establish a new SSH session on top of the given stream.
    ///
    /// The resulting SSH session is initially unauthenticated (see [`NewSession`]), and must be
    /// authenticated before it becomes useful.
    ///
    /// Note that the reactor behind the given `handle` *must* continue to be driven for any
    /// channels created from this [`Session`] to work.
    pub fn new(stream: S) -> Result<NewSession<S>, thrussh::HandlerError<()>> {
        thrussh::client::Connection::new(Arc::default(), stream, state::Ref::default(), None)
            .map(|c| NewSession {
                c: Connection { c, task: None },
            })
            .map_err(thrussh::HandlerError::Error)
    }

    fn make(c: Connection<S>) -> Self {
        let c = SharableConnection(Arc::new(Mutex::new(c)));
        Session(c)
    }

    /// Retrieve the last error encountered during this session.
    ///
    /// Note that it is unlikely you will be able to use any items associated with this session
    /// once it has returned an error.
    ///
    /// Calling this method clears the error.
    pub fn last_error(&mut self) -> Option<thrussh::HandlerError<()>> {
        let connection = (self.0).0.lock().unwrap();
        let handler = connection.c.handler();
        let mut state = handler.lock().unwrap();
        state.errored_with.take()
    }

    /// Establish a new channel over this session to execute the given command.
    ///
    /// Note that any errors encountered while operating on the channel after it has been opened
    /// will manifest only as reads or writes no longer succeeding. To get the underlying error,
    /// call [`Session::last_error`].
    pub fn open_exec<'a>(&mut self, cmd: &'a str) -> channel::ChannelOpenFuture<'a, S> {
        let mut session = (self.0).0.lock().unwrap();
        let state = session.c.handler().clone();

        let channel_id = (&mut *session.c)
            .channel_open_session()
            .expect("sessions are always authenticated");
        state
            .lock()
            .unwrap()
            .state_for
            .insert(channel_id, channel::State::default());
        channel::ChannelOpenFuture::new(cmd, self.0.clone(), state, channel_id)
    }
}
