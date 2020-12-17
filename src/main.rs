#![forbid(unsafe_code)]

use std::{
    ops::{DerefMut, Range},
    sync::mpsc::Sender,
    sync::mpsc::{Receiver, TryRecvError},
};

use client_manager::{Client, ClientAction, ClientState, Connection, ReadJsonMessageError};
use cursive::{
    theme::{Color, ColorType, Effect, Style},
    traits::Scrollable,
    traits::{Boxable, Nameable},
    view::ScrollStrategy,
    views::Dialog,
    views::EditView,
    views::LinearLayout,
    views::ResizedView,
    views::TextArea,
    Cursive, CursiveRunner,
};

use escapes::{Escaped, Escapes};
use hack_chat_types::{
    client, server, util::IntoJson, Channel, Nickname, Password, ServerApi, Text, Trip,
};
use slog::{crit, info, warn};
use slog_unwrap::{OptionExt, ResultExt};
use sloggers::Build;
use styled::{InsertMode, StyledString};
use tungstenite::{client::AutoStream, Message, WebSocket};
use url::Url;

mod client_manager;
mod escapes;
mod styled;

pub enum DisplayAction {
    /// Simple dialog display.
    DisplayDialog(String),
    CreateChat,
    /// Add a message to the current message log.
    AddChatMessage(ChatMessage),
    Exit,
    AlertReconnecting,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    /// This is a string rather than a `Nickname` as it does not neccessarily have to be
    /// any actual user's nickname.
    pub from: MessageName,
    pub trip: Option<Trip>,
    pub text: Text,
}
#[derive(Debug, Clone, PartialEq)]
pub enum MessageName {
    Server,
    ServerWarn,
    User(String),
    None,
}

const TEXT_AREA_NAME: &str = "chat_text_area";
const CHAT_AREA_NAME: &str = "chat_area";
pub struct ChatDisplay<'a> {
    pub receiver: Receiver<DisplayAction>,
    pub sender: Sender<ClientAction>,
    pub messages: Vec<ChatMessage>,
    pub log: slog::Logger,
    pub escapes: Escapes<'a>,
}
impl<'a> ChatDisplay<'a> {
    pub fn new(
        receiver: Receiver<DisplayAction>,
        sender: Sender<ClientAction>,
        escapes: Escapes<'a>,
        log: slog::Logger,
    ) -> Self {
        Self {
            receiver,
            sender,
            log,
            escapes,
            messages: Vec::with_capacity(512),
        }
    }

    fn format_sender(&self, nick: MessageName, trip: Option<String>) -> StyledString {
        const NICK_TRIP_SEPARATOR: &str = " ";
        const TEXT_SEPARATOR: &str = "| ";
        const NICKNAME_SIZE: usize = 24;
        const TRIP_SIZE: usize = 6;
        const NICK_TRIP_SEPARATOR_SIZE: usize = NICK_TRIP_SEPARATOR.len();
        const TEXT_SEPARATOR_SIZE: usize = TEXT_SEPARATOR.len();
        const SIZE: usize =
            NICKNAME_SIZE + TRIP_SIZE + NICK_TRIP_SEPARATOR_SIZE + TEXT_SEPARATOR_SIZE;

        let trip_separator = if trip.is_some() {
            NICK_TRIP_SEPARATOR
        } else {
            ""
        };
        let trip = trip.as_deref().unwrap_or("");
        let mut text = StyledString::default();
        text.append_styled(
            trip,
            Style::merge(&[
                Effect::Italic.into(),
                ColorType::Color(Color::Rgb(0x33, 0x33, 0x33)).into(),
            ]),
        );
        text.append_source(trip_separator);
        match nick {
            MessageName::None => {}
            MessageName::Server => text.append_source("*"),
            MessageName::ServerWarn => text.append_source("!"),
            MessageName::User(user) => text.append_source(user.as_str()),
        }
        text.append_source(TEXT_SEPARATOR);
        if text.len() < SIZE {
            let amount = SIZE - text.len();
            text.insert_str(0, " ".repeat(amount).as_str(), InsertMode::BreakApart);
        }
        text
    }

    pub fn handle_actions(&mut self, siv: &mut Cursive) -> bool {
        match self.receiver.try_recv() {
            Ok(action) => {
                match action {
                    DisplayAction::DisplayDialog(text) => {
                        let text = self.escapes.apply(text);
                        self.display_dialog(siv, text)
                    }
                    DisplayAction::AddChatMessage(message) => {
                        let user = self.format_sender(message.from, message.trip.map(|x| x.0));
                        let user = self.escapes.apply(user);
                        let text = self.escapes.apply(message.text);
                        self.add_message(siv, user, text);
                    }
                    DisplayAction::CreateChat => {
                        // Clone the sender, which gives us access to the same place, and allows us
                        // to take ownership of it to send actions.
                        let sender = self.sender.clone();
                        let log = self.log.clone();
                        // Create the text input area.
                        // TODO: configurable min and max dimensions.
                        let text_area = TextArea::new()
                            .with_name(TEXT_AREA_NAME)
                            .min_height(2)
                            .min_width(40)
                            .max_height(6)
                            .scrollable();
                        // Create the area where chat messages are stored.
                        let chat_area = LinearLayout::vertical()
                            .with_name(CHAT_AREA_NAME)
                            .scrollable()
                            .scroll_strategy(ScrollStrategy::StickToBottom);
                        // Create the dialog that is displayed.
                        // Displays messages (chat area) above the user input (text area)
                        let dialog = Dialog::around(
                            LinearLayout::vertical().child(chat_area).child(text_area),
                        )
                        // Handle the send button.
                        .button("Send", move |siv| {
                            siv.call_on_name(TEXT_AREA_NAME, |view: &mut TextArea| {
                                let content = view.get_content();
                                // TODO: don't panic here.
                                sender
                                    .send(ClientAction::SendChatMessage(content.to_owned()))
                                    .expect_or_log(&log, "Failed to send chat message action.");
                                view.set_content("");
                            });
                        });
                        // Create a resized view that puts this at full screen since its the main
                        // thing we're displaying.
                        let resized_view = ResizedView::with_full_screen(dialog);
                        siv.add_layer(resized_view);
                    }
                    DisplayAction::Exit => {
                        std::process::exit(0);
                    }
                    DisplayAction::AlertReconnecting => {
                        let user = self.format_sender(MessageName::Server, None);
                        let user = self.escapes.apply(user);
                        self.add_message(siv, user, self.escapes.apply("Reconnecting"));
                    }
                };
                return true;
            }
            // TODO: We could kill the thread and do complete reconnection logic.
            Err(TryRecvError::Disconnected) => {
                crit!(self.log, "Socket-thread's channel (connection between threads) was disconnected. This is fatal.");
            }
            // There was nothing to read. This is perfectly fine since as this is not blocking.
            Err(TryRecvError::Empty) => {}
        }
        // There was no actions performed.
        false
    }

    fn add_message(
        &mut self,
        siv: &mut Cursive,
        user: Escaped<StyledString>,
        text: Escaped<StyledString>,
    ) -> bool {
        if let Some(mut chat_area) = siv.find_name::<LinearLayout>(CHAT_AREA_NAME) {
            let user = escapes::create_text_view(user);
            let text = escapes::create_text_view(text);
            let message_box = LinearLayout::horizontal().child(user).child(text);
            chat_area.add_child(message_box);
            true
        } else {
            warn!(
                self.log,
                "Failed to find chat area to add chat message in '{}| {}'",
                user.into_inner().source(),
                text.into_inner().source()
            );
            false
        }
    }

    fn display_dialog<T>(&self, siv: &mut Cursive, text: Escaped<T>)
    where
        T: Into<StyledString>,
    {
        siv.add_layer(escapes::create_info_dialog(text))
    }
}

#[derive(Debug, Clone)]
enum ErrorMode {
    None,
    Reconnect,
    Exit,
}

fn main() {
    let log = {
        // Set up logging.
        let file = sloggers::file::FileLoggerBuilder::new("./log.txt")
            .build()
            .expect("Failed to start logging system.");

        let logger = slog::Logger::root(file, slog::o!());
        info!(logger, "Started logging.");
        logger
    };

    let matches = clap::App::new("Fiskar")
        .version("0.2")
        .author("MinusGix")
        .about("Hack.chat websocket client for the terminal")
        .arg(clap::Arg::with_name("username").short("u").long("username").value_name("NICK").help("Sets the username that you will join with").takes_value(true))
        .arg(clap::Arg::with_name("password").short("p").long("password").value_name("PASS").help("Sets the password that you will join with. Note that this may appear in your shell history!").takes_value(true))
        .arg(clap::Arg::with_name("channel").short("c").long("channel").value_name("CHANNEL").help("Sets the channel that you wish to join.")).get_matches();

    let nickname = matches.value_of("username");
    let password = matches.value_of("password");
    let channel = matches.value_of("channel").unwrap_or("programming");

    let mut siv = Cursive::new();

    let server_address = "wss://hack.chat/chat-ws";

    // (Client -> Display) action channel
    let (display_sender, display_receiver): (Sender<DisplayAction>, Receiver<DisplayAction>) =
        std::sync::mpsc::channel();
    let (client_sender, client_receiver): (Sender<ClientAction>, Receiver<ClientAction>) =
        std::sync::mpsc::channel();
    info!(
        log,
        "Created channels to communicate actions between socket and main thread"
    );

    let escapes = Escapes::default();

    let mut display = ChatDisplay::new(display_receiver, client_sender, escapes, log.clone());

    info!(log, "Created chat display structure");

    // This is bad code here.
    // Explanation of the following several lines code:
    // So, show_username_dialog originally took an `Fn`, but I didn't want to construct the log
    // file, server address, channels, and ChatDisplay *after* it gets username, especially as there
    // is only one ChatDisplay and socket.
    // but I can't make the function an FnOnce, because there's no way of assuring it that it will
    // only be called once by the code. I couldn't even find a way to make it do nothing if it was
    // called again. This meant that I couldn't declare things before the dialog was done, and then
    // move them into the callback as Rust did not know that the callback could/should only be
    // called a single time.
    // So, we come to here.
    // `show_username_dialog` was changed to take an `FnMut`, this would let it modify the outside
    // world, but I still needed to take ownership of values (I don't want to clone it!). So,
    // I stuffed them in Options, so they have the option of not existing or not. They should always
    // exist, and if they don't then that's a hard error as that means we were called more than
    // once.

    // TODO: verify that this clone isn't doing something wasteful.
    let mut log_opt = Some(log.clone());
    let mut display_sender = Some(display_sender);
    let mut client_receiver = Some(client_receiver);
    let mut server_address = Some(server_address);
    let mut channel = Some(Channel::from(channel));
    let mut password = password.map(Password::from);
    let mut join_as_callback = move |nick: String| {
        // TODO: make these expects log if failed
        let log = log_opt.take().expect("Failed to take ownership of log.");
        let display_sender = display_sender
            .take()
            .expect("Failed to take ownership of display sender");
        let client_receiver = client_receiver
            .take()
            .expect("Failed to take ownership of display_receiver");
        let server_address = server_address
            .take()
            .expect("Failed to take ownership of server address");
        let channel = channel.take().expect("Failed to take ownership of channel");
        // The password being None is perfectly fine.
        let password = password.take();

        // Start the thread that the socket is created upon.
        std::thread::spawn(move || {
            info!(log, "Created thread, Connecting socket");

            let connection = Connection::connect(
                display_sender,
                client_receiver,
                server_address.to_owned(),
                ServerApi::HackChatV2,
                nick.clone(),
                password,
                channel,
            )
            .expect_or_log(&log, "Failed to connect to chat.");

            info!(log, "Socket connected");

            // Set up the chat
            connection
                .action_sender
                .send(DisplayAction::CreateChat)
                .expect_or_log(
                    &log,
                    "Failed to send action telling main thread to create chat.",
                );

            let mut cli = make_client(connection, log);

            cli.con
                .send_opening_commands()
                .expect_or_log(&cli.log(), "Failed to send opening commands");

            loop {
                // Non-blocking read of json value.
                let error_mode = match cli.con.read_json_message() {
                    Ok(json) => {
                        if let Some(json) = json {
                            cli.handle_json(json).expect_or_log(
                                cli.log(),
                                "Failed to handle server-command's JSON properly.",
                            );
                        }
                        ErrorMode::None
                    }
                    Err(ReadJsonMessageError::Socket(socket_err)) => match socket_err {
                        // TODO: properly drop connection socket,
                        // TODO: Do reconnect shenanigans as well.
                        // TODO: we can inform user that these broke on most/all of these since ui
                        // is probably still alive.
                        // The connection was closed
                        tungstenite::Error::ConnectionClosed => {
                            crit!(cli.log(), "Socket connection closed");
                            ErrorMode::Reconnect
                        }
                        // The connection was closed and we're trying to mess with it!
                        tungstenite::Error::AlreadyClosed => {
                            crit!(cli.log(), "Connection was closed yet we didn't stop!");
                            ErrorMode::Reconnect
                        }
                        tungstenite::Error::Io(err) => {
                            crit!(cli.log(), "Socket I/O Error: {}", err);
                            ErrorMode::Reconnect
                        }
                        tungstenite::Error::Tls(err) => {
                            crit!(cli.log(), "Socket TLS Error: {}", err);
                            ErrorMode::Reconnect
                        }
                        // TODO: Alert user we received too large message and ignore it.
                        // unsure as to what the parameter in it is. the message?
                        tungstenite::Error::Capacity(err) => {
                            crit!(cli.log(), "Received too large message on socket: '{}'", err);
                            ErrorMode::None
                        }
                        // This may mean that we aren't connecting to socket
                        // end point. Unsure as to what the parameter is.
                        tungstenite::Error::Protocol(err) => {
                            crit!(cli.log(), "Received socket protocol error!: '{}'", err);
                            ErrorMode::Reconnect
                        }
                        // This would be impressive/worrying as the default is unlimited, but we
                        // didn't run into OOM, since rust would combust if that happened.
                        tungstenite::Error::SendQueueFull(err) => {
                            crit!(cli.log(), "The socket send queue was full: '{}'", err);
                            ErrorMode::None
                        }
                        // This is unfortunate, and I don't think this should happen?
                        tungstenite::Error::Utf8 => {
                            crit!(cli.log(), "Socket received invalid utf8");
                            ErrorMode::None
                        }
                        tungstenite::Error::Url(err) => {
                            // TODO: is this sensible?
                            crit!(cli.log(), "Invalid socket url: '{}'", err);
                            ErrorMode::Reconnect
                        }
                        tungstenite::Error::Http(status) => {
                            // TODO: is this sensible?
                            crit!(
                                cli.log(),
                                "Failed to connect, received status code: {}",
                                status
                            );
                            ErrorMode::Reconnect
                        }
                        tungstenite::Error::HttpFormat(err) => {
                            // TODO: is this sensible?
                            crit!(cli.log(), "Socket http format error: {}", err);
                            ErrorMode::Reconnect
                        }
                    },
                    // TODO: display that we got invalid json, and then ignore it.
                    Err(ReadJsonMessageError::Json(_)) => {
                        crit!(cli.log(), "Received invalid json from server");
                        ErrorMode::None
                    }
                };
                // If we dced then do a while loop using sleep to make so we wait until timeout is
                // done to try reconnecting?
                match error_mode {
                    ErrorMode::None => {}
                    ErrorMode::Reconnect => {
                        loop {
                            // Sleep for a bit before reconnecting.
                            cli.con
                                .act(DisplayAction::AlertReconnecting)
                                .expect_or_log(&cli.log(), "Failed to send reconnecting message");
                            std::thread::sleep(cli.timeout);
                            if let Err(_err) = cli.con.reconnect() {
                                // Ignore and so we reloop and try reconnecting.
                            } else {
                                // Send the opening salvo
                                cli.con
                                    .send_opening_commands()
                                    .expect_or_log(&cli.log(), "Failed to send opening salvo");
                                // Break out of the loop since we have reconnected.
                                break;
                            }
                        }
                        // Skip past action processing after reconnect.
                        continue;
                    }
                    ErrorMode::Exit => {
                        cli.con.act(DisplayAction::Exit).expect_or_log(
                            &cli.log(),
                            "Failed to send exit action over channel to main thread",
                        );
                        // Break out of the loop so the socket thread ends.
                        break;
                    }
                };

                // Handle actions sent by Display, non-blocking.

                let con = &mut cli.con;
                let log = &mut cli.state.log;
                let action_receiver = &mut con.action_receiver;
                let socket = &mut con.socket;
                for action in action_receiver.try_iter() {
                    match action {
                        ClientAction::SendChatMessage(text) => {
                            let msg = client::Chat {
                                channel: Some(con.channel.clone()),
                                text,
                            };
                            // TODO: it'd be nice not to have to manually send whilst processing
                            // actions
                            // TODO: don't panic if we failed to send!
                            socket
                                .write_message(Message::Text(msg.into_json(con.server_api).dump()))
                                .expect_or_log(log, "Failed to send chat message.")
                        }
                    };
                }
            }
        });
    };

    if let Some(nickname) = nickname {
        join_as_callback(nickname.to_owned());
    } else {
        let join_dialog = show_username_dialog(log.clone(), join_as_callback);
        siv.add_layer(join_dialog);
    }

    let backend = cursive::backends::curses::n::Backend::init().unwrap();
    let mut runner = siv.runner(backend);

    runner.refresh();
    // Set up view, drawing it to the screen.

    while runner.is_running() {
        let ran_action = display.handle_actions(runner.deref_mut());

        // Passing in true to `post_events` will cause it to call refresh in a normal manner, so it
        // is essentially the same as calling refresh ourselves. This might also avoid two draws on
        // any update?
        let received_something = runner.process_events() || ran_action;
        runner.post_events(received_something);
    }
}

fn show_username_dialog<F>(log: slog::Logger, mut cb: F) -> Dialog
where
    F: FnMut(String) + 'static,
{
    const USERNAME_INPUT_NAME: &str = "joining-username-input";
    let receive_username = move |siv: &mut Cursive, name: &str| {
        if name.is_empty() {
            siv.add_layer(Dialog::info("Please enter a username!"));
        } else {
            // Get rid of the input box.
            // TODO: may be able to take the String from this so that we don't have to do another
            // heap allocation
            siv.pop_layer();
            cb(name.to_owned());
        }
    };

    // The maximum hack.chat nickname length is 24, but we don't restrict that, merely making the
    // width the usual size.
    // It is 25 instead of 24 as if you type 24 characters, the input box 'slides over'
    let username_input = EditView::new()
        .on_submit_mut(receive_username)
        .with_name(USERNAME_INPUT_NAME)
        .fixed_width(25);
    Dialog::new().title("Username").content(username_input)
    // .button("Ok", move |siv| {
    //     let name = siv
    //         .call_on_name(USERNAME_INPUT_NAME, |view: &mut EditView| {
    //             // TODO: make this call on_submit
    //         })
    //         .expect_or_log(&log, "Expected name field to exist");
    // })
}

fn make_client(connection: Connection, log: slog::Logger) -> Client {
    let mut client = Client::new(connection, ClientState { log });

    client.handlers.online_set.addg(|con, state, cmd| {
        let text = if let Some(nicks) = &cmd.nicks {
            let mut text = String::with_capacity(nicks.len() * 10);
            text += "Online Users: ";
            for nick in nicks {
                text += &nick;
                text += ", ";
            }
            text
        } else {
            "[Failed to acquire nicknames on user join]".to_owned()
        };
        con.act(DisplayAction::AddChatMessage(ChatMessage {
            from: MessageName::Server,
            trip: None,
            text,
        }))
        .expect_or_log(&state.log, "Failed to send online set action");
    });
    client.handlers.chat.addg(|con, state, cmd| {
        con.act(DisplayAction::AddChatMessage(ChatMessage {
            from: MessageName::User(cmd.nick.clone()),
            trip: cmd.trip.clone().into(),
            text: cmd.text.clone(),
        }))
        .expect_or_log(&state.log, "Failed to send chat message action");
    });
    // client.handlers.session.addg(|_con, _state, _cmd| {
    //     // TODO: tell user of session information?
    // });
    client.handlers.info.addg(|con, state, cmd| {
        con.act(DisplayAction::AddChatMessage(ChatMessage {
            from: MessageName::Server,
            trip: None,
            text: cmd.text.clone(),
        }))
        .expect_or_log(&state.log, "Failed to send info action");
    });
    client.handlers.captcha.addg(|con, state, cmd| {
        con.act(DisplayAction::AddChatMessage(ChatMessage {
            from: MessageName::Server,
            trip: None,
            text: cmd.text.clone(),
        }))
        .expect_or_log(&state.log, "Failed to send captcha action");
    });
    client.handlers.emote.addg(|con, state, cmd| {
        // TODO: make this use the actual user's nick.
        con.act(DisplayAction::AddChatMessage(ChatMessage {
            from: MessageName::Server,
            trip: None,
            text: cmd.text.clone(),
        }))
        .expect_or_log(&state.log, "Failed to send emote related action");
    });
    client.handlers.invite.addg(|con, state, cmd| {
        // TODO: tell them if it was them using 'You' rather than their own nick.
        let from = con
            .users
            .get(cmd.from)
            .map(|x| x.nick.as_ref())
            .unwrap_or("[UNKNOWN]");
        let to = con
            .users
            .get(cmd.to)
            .map(|x| x.nick.as_ref())
            .unwrap_or("[UNKOWN]");
        con.action_sender
            .send(DisplayAction::AddChatMessage(ChatMessage {
                from: MessageName::Server,
                trip: None,
                text: format!("{} invited {} to ?{}", from, to, cmd.invite_channel),
            }))
            .expect_or_log(&state.log, "Failed to send invite related action");
    });
    client.handlers.online_add.addg(|con, state, cmd| {
        con.act(DisplayAction::AddChatMessage(ChatMessage {
            from: MessageName::Server,
            trip: None,
            text: format!("{} joined", cmd.nick),
        }))
        .expect_or_log(&state.log, "Failed to send online add related action");
    });
    client.handlers.online_remove.addg(|con, state, cmd| {
        con.act(DisplayAction::AddChatMessage(ChatMessage {
            from: MessageName::Server,
            trip: None,
            text: format!("{} left", cmd.nick),
        }))
        .expect_or_log(&state.log, "Failed to send online remove related action");
    });
    client.handlers.warn.addg(|con, state, cmd| {
        con.act(DisplayAction::AddChatMessage(ChatMessage {
            from: MessageName::ServerWarn,
            trip: None,
            text: cmd.text.clone(),
        }))
        .expect_or_log(&state.log, "Failed to send warn related action");
    });

    client
}
