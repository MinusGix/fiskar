use std::{
    sync::mpsc::{Receiver, SendError, Sender},
    time,
};

use hack_chat_types::{
    client, id, server, util::ClientCommand, util::Command, util::FromJson, util::FromJsonError,
    util::IntoJson, util::MaybeExist, AccessUserId, Channel, Nickname, Password, ServerApi,
    SessionId, UserInfo, Users,
};
use json::JsonValue;
use slog::{crit, warn};
use tungstenite::{
    client::{AutoStream, IntoClientRequest},
    util::NonBlockingResult,
    Message, WebSocket,
};
use url::Url;

use crate::DisplayAction;

// FIXME: Implement client action and use non-blocking methods so that we can check the event loop in the thread.
pub enum ClientAction {
    SendChatMessage(String),
}

#[derive(Debug)]
pub enum ReadJsonMessageError {
    Socket(tungstenite::Error),
    Json(json::JsonError),
}
impl From<tungstenite::Error> for ReadJsonMessageError {
    fn from(err: tungstenite::Error) -> Self {
        ReadJsonMessageError::Socket(err)
    }
}
impl From<json::JsonError> for ReadJsonMessageError {
    fn from(err: json::JsonError) -> Self {
        ReadJsonMessageError::Json(err)
    }
}

pub struct Connection {
    /// A destination to send DisplayActions to the main thread that we wish to have performed
    pub action_sender: Sender<DisplayAction>,
    /// A place to receive ClientActions from the main thread that it wishes to have performed
    pub action_receiver: Receiver<ClientAction>,
    /// The socket that is connected to the chat.
    pub socket: WebSocket<AutoStream>,
    /// Decides how to interpret and send certain data between the client and server.
    pub server_api: ServerApi,
    /// Keep track of tthe users
    pub users: Users,
    /// V2 session id of the client, if applicable
    session_id: Option<SessionId>,
    /// The address of the server
    pub address: String,
    /// The nickname that was joined with.
    pub joined_nick: Nickname,
    /// The password that was used
    pub password: Option<Password>,
    /// The channel that was joined.
    pub channel: Channel,
}
impl Connection {
    pub fn new(
        action_sender: Sender<DisplayAction>,
        action_receiver: Receiver<ClientAction>,
        socket: WebSocket<AutoStream>,
        address: String,
        server_api: ServerApi,
        nick: Nickname,
        password: Option<Password>,
        channel: Channel,
    ) -> Self {
        Self {
            server_api,
            action_sender,
            action_receiver,
            socket,
            joined_nick: nick,
            password,
            address,
            channel,
            session_id: None,
            users: Users::default(),
        }
    }

    pub fn connect(
        action_sender: Sender<DisplayAction>,
        action_receiver: Receiver<ClientAction>,
        address: String,
        server_api: ServerApi,
        nick: Nickname,
        password: Option<Password>,
        channel: Channel,
    ) -> tungstenite::Result<Self> {
        let (socket, _response) = tungstenite::connect(address.as_str())?;
        Ok(Self::new(
            action_sender,
            action_receiver,
            socket,
            address,
            server_api,
            nick,
            password,
            channel,
        ))
    }

    /// Recreates the socket.
    /// Note that it does _not_ send the opening salvo.
    pub fn reconnect(&mut self) -> tungstenite::Result<()> {
        let (socket, _response) = tungstenite::connect(self.address.as_str())?;
        self.socket = socket;
        Ok(())
    }

    /// This is meant to register handlers relating directly to the connection.
    /// The most notable of that being tracking the userid -> username mapping
    /// that the v2 server requires.
    pub fn register_handlers(handlers: &mut CommandHandlers<ClientState>) {
        // TODO: for many of the functions we could work to grab as much info as possible from them
        // to keep track of user information for later use. (Because, the server, especially on
        // legacy, does not provide us all pertinent information (trips, hashes) on join.)
        handlers.session.addg(|con, _, session| {
            // TODO: log if we already had a session id and are getting a new one.
            // We are forced to clone the session id here rather than taking ownership of it because
            // of not receiving ownership of the session command.
            // Which makes sense, but is a slightly sad inefficiency, since much of the time other
            // code doesn't care about the session command, and if they needed the session id
            // they could get it from their access to the connection.
            con.session_id = Some(session.session_id.clone());
        });

        handlers.online_set.addg(|con, state, online_set| {
            // TODO: log a note if the channel is different than the one we joined.
            // We clear the tracked users as they have been set.
            // As the online set command is only ran when the client connects.
            con.users.clear();
            if let Some(users) = &online_set.users {
                let mut found_self = false;
                let mut found_self_from_me_field = false;
                for user in users {
                    // Get the user id attached to the user, if it doesn't exist then generate an
                    // id.
                    let user_id = user
                        .user_id
                        .map(AccessUserId::Server)
                        .unwrap_or_else(|| con.users.generate_id());

                    let nick = user.nick.clone();

                    let trip = user.trip.clone();

                    // TODO: check if only some fields have is_me and alert if so?
                    // TODO: check if found_self was previously set, and log an alert.
                    if let Some(is_me) = user.is_me {
                        if is_me {
                            // It is declared to be this connection, thus we store it as ourself.
                            con.users.ourself = Some(user_id);
                            found_self = true;
                            found_self_from_me_field = true;
                        }
                    } else {
                        // It doesn't even have the option, so we simply check if the nickname was
                        // the one we joined with
                        if nick == con.joined_nick {
                            found_self = true;
                            found_self_from_me_field = false;
                            con.users.ourself = Some(user_id);
                        }
                    }

                    con.users.insert(
                        user_id,
                        UserInfo {
                            nick,
                            trip,
                            online: true,
                        },
                    );
                }

                if !found_self {
                    // TODO: alert that we failed to find ourself in the user list, and that this
                    // may be a sign of a possibly unknown API setup.
                    // We manually add ourselves to the listing for now.
                    let user_id = con.users.generate_id();
                    con.users.insert(
                        user_id,
                        UserInfo {
                            nick: con.joined_nick.clone(),
                            // We don't know the trip.
                            trip: MaybeExist::Unknown,
                            // Iffy.
                            online: true,
                        },
                    );
                }
            } else if let Some(nicks) = &online_set.nicks {
                let mut found_self = false;
                for nick in nicks {
                    // Since we did not receive a user id
                    let user_id = con.users.generate_id();

                    if nick == &con.joined_nick {
                        // TODO: log if we found ourself twice.
                        found_self = true;
                        con.users.ourself = Some(user_id);
                    }

                    con.users.insert(
                        user_id,
                        UserInfo {
                            nick: nick.clone(),
                            // We don't know what their trip is.
                            trip: MaybeExist::Unknown,
                            online: true,
                        },
                    );
                }

                if !found_self {
                    // TODO: log that we failed to find ourselves.
                    // We give ourselves an id.
                    let user_id = con.users.generate_id();
                    con.users.insert(
                        user_id,
                        UserInfo {
                            nick: con.joined_nick.clone(),
                            // We don't know what our trip is
                            trip: MaybeExist::Unknown,
                            // Iffy
                            online: true,
                        },
                    )
                }
            } else {
                // TODO: Log error in this case.
                crit!(state.log, "Did not receive any user information from onlineSet. This could be quite bad for behavior of program.");
            }
        });

        handlers.online_add.addg(|con, _, add| {
            // TODO: if channel is wrong then comment that the channel is incorrect
            let user_id = add
                .user_id
                .map(AccessUserId::Server)
                .unwrap_or_else(|| con.users.generate_id());

            con.users.insert(
                user_id,
                UserInfo {
                    nick: add.nick.clone(),
                    trip: add.trip.clone(),
                    online: true,
                },
            )
        });

        handlers.online_remove.addg(|con, _, remove| {
            let user_id = remove
                .user_id
                .map(AccessUserId::Server)
                .or_else(|| con.users.find_online_nick(&remove.nick).map(|x| x.0));

            let user_id = if let Some(user_id) = user_id {
                user_id
            } else {
                // TODO: log that we failed to get access id of user that left.
                return;
            };

            let info = if let Some(info) = con.users.get_mut(user_id) {
                info
            } else {
                // TODO: log that we failed to user id. Perhaps mention whether it was on cmd.
                return;
            };

            info.online = false;
        });
    }

    /// Send an action to be performed over the channel.
    pub fn act(&mut self, action: DisplayAction) -> Result<(), SendError<DisplayAction>> {
        self.action_sender.send(action)
    }

    // TODO: handle the possibility of the send queue being full.
    /// Send a websocket Client command to the server. Maybe blocking?
    pub fn send<T>(&mut self, message: T) -> Result<(), tungstenite::Error>
    where
        T: Sized + ClientCommand + IntoJson,
    {
        let message = message.into_json(self.server_api).dump();
        self.socket.write_message(Message::Text(message))
    }

    // TODO: handle closing error from this
    // TODO: call write_pending ourselves to advance it?
    /// Read a message from the server. Non-blocking.
    pub fn read_message(&mut self) -> Result<Option<Message>, tungstenite::Error> {
        self.socket.read_message().no_block()
    }

    /// Read a message as json from the server, ignoring the rest. Non-blocking.
    pub fn read_json_message(&mut self) -> Result<Option<JsonValue>, ReadJsonMessageError> {
        let message = self.read_message()?;
        if let Some(Message::Text(text)) = message {
            Ok(Some(json::parse(&text)?))
        } else {
            Ok(None)
        }
    }

    pub fn send_opening_commands(&mut self) -> Result<(), tungstenite::Error> {
        if self.server_api == ServerApi::HackChatV2 {
            self.send(client::Session {
                id: None,
                is_bot: false,
            })?;
        }

        self.send(client::Join {
            nick: self.joined_nick.clone(),
            channel: self.channel.clone(),
            password: self.password.clone(),
        })?;

        Ok(())
    }
}

// NOTE: This requires a connection reference rather than being completely generic as Rust can be a
// pain.
// If you have a structure that is generic (aka CommandHandlers, but that goes to this Handler)
// then if want to have a single type parameter that is user-defined parameters to the function
// then if they want to pass in a tuple, to allow for multiple arguments, they for some
// forsaken reason _have_ to give lifetime parameters.
// Ex: struct Thing<T> { handler: Handler<T, Other>, }
// Use: struct Alpha { thing: Thing<(u64, &mut File)>, file: File }
//                                         ^-- required lifetime here!
// I ran into the issue that if I specified the lifetime, then I also had to give a lifetime to my
// structure in the function that was calling it or a lifetime that lived at least as long as it.
// Ex: fn do_thing<'a>(&'a self) { (self.thing.handler)((5, &mut self.file)) }
// But for some reason Rust could not figure out that the function (handler) that was called would
// not keep the value given and so I could no longer use the fields that were given over.
// I'm not sure why all generics are required to have lifetimes, if they aren't used in a location
// where that is needed. If this had variadic types that could have also helped avoid the issue
//   (though it could still be ran into if it was still required...)
// As well, since the extra `T` parameter (custom state) is also generic, I have to at least specify
// `&mut` upon it, otherwise I would be forced to specify a lifetime as well.
// Having a way to map a tuple of types to a tuple of (mut-)references to those types would be nice.
pub type Handler<T, C> = Box<dyn Fn(&mut Connection, &mut T, &C)>;
pub struct HandlerList<T, C> {
    handlers: Vec<Handler<T, C>>,
}
impl<T, C> HandlerList<T, C> {
    /// Calls each handler in order.
    /// Returns `true` if any amount of handlers were called.
    pub fn call(&self, con: &mut Connection, v: &mut T, c: &C) -> bool {
        for handler in self.handlers.iter() {
            (handler)(con, v, c)
        }

        !self.handlers.is_empty()
    }

    pub fn add(&mut self, handler: Handler<T, C>) {
        self.handlers.push(handler)
    }

    pub fn addg<F>(&mut self, func: F)
    where
        F: 'static + Fn(&mut Connection, &mut T, &C),
    {
        self.add(Box::new(func))
    }
}
impl<T, C> Default for HandlerList<T, C> {
    fn default() -> Self {
        Self {
            handlers: Vec::with_capacity(2),
        }
    }
}
pub struct CommandHandlers<T>
where
    T: Sized,
{
    // TODO; handlers for raw invite and emote commands, which we can do since we're only passing
    // references
    pub session: HandlerList<T, server::Session>,
    pub online_set: HandlerList<T, server::OnlineSet>,
    pub info: HandlerList<T, server::Info>,
    pub chat: HandlerList<T, server::Chat>,
    pub captcha: HandlerList<T, server::Captcha>,
    pub emote: HandlerList<T, server::synthetic::Emote>,
    pub invite: HandlerList<T, server::synthetic::Invite>,
    pub online_add: HandlerList<T, server::OnlineAdd>,
    pub online_remove: HandlerList<T, server::OnlineRemove>,
    pub warn: HandlerList<T, server::Warn>,
}
impl<T> Default for CommandHandlers<T>
where
    T: Sized,
{
    fn default() -> Self {
        CommandHandlers {
            session: HandlerList::default(),
            online_set: HandlerList::default(),
            info: HandlerList::default(),
            chat: HandlerList::default(),
            captcha: HandlerList::default(),
            emote: HandlerList::default(),
            invite: HandlerList::default(),
            online_add: HandlerList::default(),
            online_remove: HandlerList::default(),
            warn: HandlerList::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum HandleCommandError {
    FromJson(FromJsonError),
    EmoteConversion(server::synthetic::EmoteConversionError),
}
impl From<FromJsonError> for HandleCommandError {
    fn from(err: FromJsonError) -> Self {
        Self::FromJson(err)
    }
}
impl From<server::synthetic::EmoteConversionError> for HandleCommandError {
    fn from(err: server::synthetic::EmoteConversionError) -> Self {
        Self::EmoteConversion(err)
    }
}

pub struct Client {
    pub con: Connection,
    pub handlers: CommandHandlers<ClientState>,
    pub state: ClientState,
    /// The amount of time we're sleeping in between connection attempts.
    pub timeout: time::Duration,
}
impl Client {
    pub fn new(con: Connection, state: ClientState) -> Self {
        let mut handlers = CommandHandlers::default();
        Connection::register_handlers(&mut handlers);
        Self {
            con,
            state,
            handlers,
            // 500ms
            timeout: time::Duration::from_millis(500),
        }
    }

    pub fn handle_json(&mut self, json: JsonValue) -> Result<(), HandleCommandError> {
        let cmd = json[id::CMD].as_str();
        if let Some(cmd) = cmd {
            let server_api = self.con.server_api;
            let state = &mut self.state;
            let con = &mut self.con;
            // TODO: add the rest of the commands
            // TODO: add synthesized commands.
            let _ran_cmd = match cmd {
                server::Session::CMD => self.handlers.session.call(
                    con,
                    state,
                    &server::Session::from_json(json, server_api)?,
                ),
                server::OnlineSet::CMD => self.handlers.online_set.call(
                    con,
                    state,
                    &server::OnlineSet::from_json(json, server_api)?,
                ),
                server::Info::CMD => {
                    let info = server::Info::from_json(json, server_api)?;
                    // Break apart info into separate commands.
                    if let Ok(invite) = server::synthetic::Invite::from_info(&con.users, &info) {
                        self.handlers.invite.call(con, state, &invite)
                    } else if let Ok(emote) = server::synthetic::Emote::from_info(&con.users, &info)
                    {
                        self.handlers.emote.call(con, state, &emote)
                    } else {
                        self.handlers.info.call(con, state, &info)
                    }
                }
                server::Chat::CMD => {
                    self.handlers
                        .chat
                        .call(con, state, &server::Chat::from_json(json, server_api)?)
                }
                server::OnlineAdd::CMD => self.handlers.online_add.call(
                    con,
                    state,
                    &server::OnlineAdd::from_json(json, server_api)?,
                ),
                server::OnlineRemove::CMD => self.handlers.online_remove.call(
                    con,
                    state,
                    &server::OnlineRemove::from_json(json, server_api)?,
                ),
                server::Captcha::CMD => self.handlers.captcha.call(
                    con,
                    state,
                    &server::Captcha::from_json(json, server_api)?,
                ),
                server::Invite::CMD => {
                    let invite = server::Invite::from_json(json, server_api)?;
                    let invite = server::synthetic::Invite::from_invite(&con.users, invite);
                    self.handlers.invite.call(con, state, &invite)
                }
                server::Emote::CMD => {
                    let emote = server::Emote::from_json(json, server_api)?;
                    let emote = server::synthetic::Emote::from_emote(&con.users, &emote)?;
                    self.handlers.emote.call(con, state, &emote)
                }
                server::Warn::CMD => {
                    // TODO: break warn down into component 'commands' like ratelimit and such
                    self.handlers
                        .warn
                        .call(con, state, &server::Warn::from_json(json, server_api)?)
                }
                _ => {
                    // We ignore the command.
                    warn!(
                        self.log(),
                        "Unhandled command from websocket: '{}', JSON: '{:?}'",
                        id::CMD,
                        json.pretty(2)
                    );
                    // TODO: log that we got an unknown command value
                    false
                }
            };
        } else {
            warn!(self.log(), "Received command from websocket server without a '{}' field for identification. JSON: '{:?}'", id::CMD, json.pretty(2));
        }
        Ok(())
    }

    pub fn log(&self) -> &slog::Logger {
        &self.state.log
    }
}

pub struct ClientState {
    pub log: slog::Logger,
}
impl ClientState {}
