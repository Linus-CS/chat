use std::{
    collections::HashMap,
    sync::{atomic::AtomicUsize, Arc, RwLock},
};

use actix::{Actor, ActorContext, AsyncContext, Handler, Message, Recipient, StreamHandler};
use actix_files as fs;
use actix_web::{get, http::StatusCode, web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;
use tokio::{fs::File, io::AsyncReadExt};

type Clients = Arc<RwLock<HashMap<String, Recipient<Msg>>>>;

#[derive(Message)]
#[rtype(result = "()")]
struct Msg(String);

struct WsSession {
    clients: Clients,
    name: String,
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
                if txt.starts_with("/rename") {
                    let new_name = txt.split_ascii_whitespace().nth(1).unwrap_or(&self.name);
                    if self.clients.read().unwrap().contains_key(new_name) {
                        ctx.text(format!("Name {} is already taken", new_name));
                        return;
                    }
                    let mut clients = self.clients.write().unwrap();
                    clients.remove(&self.name);
                    self.name = new_name.to_string();
                    clients.insert(self.name.clone(), ctx.address().recipient());
                    ctx.text(format!("Renamed to {} ", self.name));
                    return;
                }
                let mut clients = self.clients.write().unwrap();
                for client in clients.values_mut() {
                    client.do_send(Msg(format!("{}: {}", self.name, txt)));
                }
            }
            ws::Message::Close(_) => {
                ctx.stop();
            }
            _ => (),
        }
    }
}

async fn connect(
    req: HttpRequest,
    stream: web::Payload,
    cnt: web::Data<AtomicUsize>,
    clients: web::Data<RwLock<HashMap<String, Recipient<Msg>>>>,
) -> Result<HttpResponse, Error> {
    let id = cnt
        .into_inner()
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = "User#".to_string() + &id.to_string();
    ws::start(
        WsSession {
            name,
            clients: clients.into_inner(),
        },
        &req,
        stream,
    )
}

#[get("/index.html")]
async fn index() -> Result<HttpResponse, Error> {
    let mut file = File::open("./static/index.html").await?;
    let mut contents = vec![];
    file.read_to_end(&mut contents).await?;

    let cfg = minify_html_onepass::Cfg {
        minify_css: true,
        minify_js: true,
    };
    let new_len = minify_html_onepass::in_place(contents.as_mut_slice(), &cfg)
        .expect("Failed to minify html");
    contents.truncate(new_len);
    Ok(HttpResponse::build(StatusCode::OK)
        .content_type("text/html; charset=utf-8")
        .body(contents))
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let cnt = Arc::new(AtomicUsize::new(0));
    let clients = Clients::default();

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
