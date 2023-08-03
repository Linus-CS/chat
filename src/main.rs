use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc, RwLock},
};

use actix::{Actor, ActorContext, AsyncContext, Handler, Message, Recipient, StreamHandler};
use actix_files as fs;
use actix_web::{get, http::StatusCode, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws::{self, WebsocketContext};
use minify_html_onepass as minify;
use serde::Serialize;

type Clients = RwLock<HashMap<String, Recipient<Msg>>>;

#[derive(Message, Serialize)]
#[rtype(result = "()")]
struct Msg(String);
struct WsSession {
    clients: Arc<Clients>,
    name: String,
    color: String,
}

trait ColoredText {
    fn text_color(&mut self, text: &str, color: &str);
    fn text_black(&mut self, text: &str);
}

impl ColoredText for ws::WebsocketContext<WsSession> {
    fn text_color(&mut self, text: &str, color: &str) {
        self.text(format!(r#"["{text}","{color}"]"#));
    }
    fn text_black(&mut self, text: &str) {
        self.text_color(text, "#000000");
    }
}

impl Actor for WsSession {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let recipient = ctx.address().recipient();
        let mut clients = self.clients.write().unwrap();
        clients.insert(self.name.clone(), recipient);
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        let mut clients = self.clients.write().unwrap();
        clients.remove(&self.name);
    }
}

impl Handler<Msg> for WsSession {
    type Result = ();

    fn handle(&mut self, msg: Msg, ctx: &mut Self::Context) {
        ctx.text(msg.0);
    }
}

impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsSession {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        let msg = match msg {
            Err(_) => {
                ctx.stop();
                return;
            }
            Ok(msg) => msg,
        };

        match msg {
            ws::Message::Text(text) => {
                let txt = text.trim();
                if txt.starts_with('/') {
                    let mut cmd = txt.strip_prefix('/').unwrap().split_ascii_whitespace();
                    let name = cmd.next();
                    let args = cmd.collect::<Vec<_>>();
                    handle_cmd(name, args, self, ctx);
                    return;
                }
                let mut clients = self.clients.write().unwrap();
                for client in clients.values_mut() {
                    client.do_send(Msg(format!(
                        r#"["{}: {}", "{}"]"#,
                        self.name,
                        txt,
                        self.color.clone()
                    )));
                }
            }
            ws::Message::Close(_) => {
                ctx.stop();
            }
            _ => (),
        }
    }
}

fn handle_cmd(
    name: Option<&str>,
    args: Vec<&str>,
    session: &mut WsSession,
    ctx: &mut WebsocketContext<WsSession>,
) {
    let available_cmds = "Available commands: help, list, rename <new_name>, set_color <color>";

    match name {
        Some("help") => ctx.text_black(available_cmds),
        Some("list") => {
            let names: Vec<_> = session.clients.read().unwrap().keys().cloned().collect();
            ctx.text_black(&format!("Users: {}", names.join(", ")));
        }
        Some("rename") => match args.first() {
            Some(new_name) => {
                if session.clients.read().unwrap().contains_key(*new_name) {
                    ctx.text_black(&format!("Name {new_name} is already taken"));
                    return;
                }
                let mut clients = session.clients.write().unwrap();
                clients.remove(&session.name);
                session.name = new_name.to_string();
                clients.insert(session.name.clone(), ctx.address().recipient());
                ctx.text_black(&format!("Renamed to {} ", session.name));
            }
            _ => ctx.text_black("Usage: /rename <new_name>"),
        },
        Some("set_color") => match args.first() {
            Some(new_color) => {
                session.color = new_color.to_string();
                ctx.text_black(&format!("Color set to {} ", session.color));
            }
            _ => ctx.text_black("Usage: /set_color <color>"),
        },
        Some(cmd) => ctx.text_black(&format!(
            "Unknown command: {cmd} type /help to list avilable commands."
        )),
        _ => ctx.text_black(available_cmds),
    }
}

async fn connect(
    req: HttpRequest,
    stream: web::Payload,
    cnt: web::Data<AtomicUsize>,
    clients: web::Data<Clients>,
) -> Result<HttpResponse, Error> {
    let id = cnt
        .into_inner()
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = "User#".to_owned() + &id.to_string();
    ws::start(
        WsSession {
            name,
            clients: clients.into_inner(),
            color: "#000000".to_owned(),
        },
        &req,
        stream,
    )
}

#[get("/index.html")]
async fn index() -> Result<HttpResponse, Error> {
    minify_html("./static/index.html").await
}

async fn minify_html(path: &str) -> Result<HttpResponse, Error> {
    let mut content = tokio::fs::read(path).await?;
    let cfg = minify::Cfg {
        minify_css: true,
        minify_js: true,
    };
    let new_len = minify::in_place(content.as_mut_slice(), &cfg).expect("Failed to minify html");
    content.truncate(new_len);
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(content))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let cnt = Arc::new(AtomicUsize::new(0));
    let clients = Arc::new(Clients::default());

    let address = "0.0.0.0";
    let port = 8080;
    println!("Listening at http://{address}:{port}");
    HttpServer::new(move || {
        App::new()
            .service(web::redirect("/", "/index.html"))
            .service(index)
            .app_data(web::Data::from(cnt.clone()))
            .app_data(web::Data::from(clients.clone()))
            .route("/connect", web::get().to(connect))
            .service(fs::Files::new("/", "./static/"))
    })
    .bind((address, port))?
    .run()
    .await
}
