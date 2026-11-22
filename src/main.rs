use nu_plugin::{MsgPackSerializer, serve_plugin};
use nu_plugin_web::WebPlugin;

fn main() {
    serve_plugin(&WebPlugin, MsgPackSerializer);
}
