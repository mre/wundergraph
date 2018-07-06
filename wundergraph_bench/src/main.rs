#![deny(warnings, missing_debug_implementations, missing_copy_implementations)]
// Clippy lints
#![cfg_attr(feature = "clippy", allow(unstable_features))]
#![cfg_attr(feature = "clippy", feature(plugin))]
#![cfg_attr(feature = "clippy", plugin(clippy(conf_file = "../../clippy.toml")))]
#![cfg_attr(
    feature = "clippy",
    allow(option_map_unwrap_or_else, option_map_unwrap_or, match_same_arms, type_complexity)
)]
#![cfg_attr(
    feature = "clippy",
    warn(
        option_unwrap_used, result_unwrap_used, wrong_pub_self_convention, mut_mut,
        non_ascii_literal, similar_names, unicode_not_nfc, enum_glob_use, if_not_else,
        items_after_statements, used_underscore_binding
    )
)]

#[macro_use]
extern crate diesel;
#[macro_use]
extern crate juniper;
extern crate actix;
extern crate actix_web;
#[macro_use]
extern crate wundergraph;
extern crate failure;
#[macro_use]
extern crate serde;
extern crate chrono;
extern crate env_logger;
extern crate futures;
extern crate num_cpus;
extern crate serde_json;

use actix::{Actor, Addr, Handler, Message, Syn, SyncArbiter, SyncContext};
use actix_web::{
    http, server, App, AsyncResponder, FutureResponse, HttpRequest, HttpResponse, Json, State,
};
use diesel::pg::PgConnection;
use diesel::r2d2::{ConnectionManager, Pool};
use failure::Error;
use futures::Future;
use juniper::graphiql::graphiql_source;
use juniper::http::GraphQLRequest;
use std::env;
use std::sync::Arc;

mod api;

type Schema = juniper::RootNode<
    'static,
    self::api::Query<Pool<ConnectionManager<PgConnection>>>,
    self::api::Mutation<Pool<ConnectionManager<PgConnection>>>,
>;

// actix integration stuff
#[derive(Serialize, Deserialize, Debug)]
pub struct GraphQLData(GraphQLRequest);

impl Message for GraphQLData {
    type Result = Result<String, Error>;
}

#[allow(missing_debug_implementations)]
pub struct GraphQLExecutor {
    schema: Arc<Schema>,
    pool: Arc<Pool<ConnectionManager<PgConnection>>>,
}

impl GraphQLExecutor {
    fn new(
        schema: Arc<Schema>,
        pool: Arc<Pool<ConnectionManager<PgConnection>>>,
    ) -> GraphQLExecutor {
        GraphQLExecutor { schema, pool }
    }
}

impl Actor for GraphQLExecutor {
    type Context = SyncContext<Self>;
}

impl Handler<GraphQLData> for GraphQLExecutor {
    type Result = Result<String, Error>;

    fn handle(&mut self, msg: GraphQLData, _: &mut Self::Context) -> Self::Result {
        let ctx = self.pool.get()?;
        //        let ctx = MyContext::new(self.pool.get()?);
        let res = msg.0.execute(&self.schema, &ctx);
        let res_text = serde_json::to_string(&res)?;
        Ok(res_text)
    }
}

struct AppState {
    executor: Addr<Syn, GraphQLExecutor>,
}

#[cfg_attr(feature = "clippy", allow(needless_pass_by_value))]
fn graphiql(_req: HttpRequest<AppState>) -> Result<HttpResponse, Error> {
    let html = graphiql_source("/graphql");
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html))
}

#[cfg_attr(feature = "clippy", allow(needless_pass_by_value))]
fn graphql(st: State<AppState>, data: Json<GraphQLData>) -> FutureResponse<HttpResponse> {
    st.executor
        .send(data.0)
        .from_err()
        .and_then(|res| match res {
            Ok(res) => Ok(HttpResponse::Ok()
                .content_type("application/json")
                .body(res)),
            Err(_) => Ok(HttpResponse::InternalServerError().into()),
        })
        .responder()
}

fn main() {
    //   env::set_var("RUST_LOG", "actix_web=info");
    //   env_logger::init();
    let manager = ConnectionManager::<PgConnection>::new(
        env::var("DATABASE_URL").expect("Database url not set"),
    );
    let pool = Pool::builder()
        .max_size((num_cpus::get() * 2 * 4) as u32)
        .build(manager)
        .expect("Failed to init pool");

    let query = self::api::Query::<Pool<ConnectionManager<PgConnection>>>::default();
    let mutation = self::api::Mutation::<Pool<ConnectionManager<PgConnection>>>::default();
    let schema = Schema::new(query, mutation);

    let sys = actix::System::new("wundergraph-bench");

    let schema = Arc::new(schema);
    let pool = Arc::new(pool);
    let addr = SyncArbiter::start(num_cpus::get() + 1, move || {
        GraphQLExecutor::new(schema.clone(), pool.clone())
    });
    let url = env::var("URL").unwrap_or_else(|_| String::from("127.0.0.1:8000"));

    // Start http server
    server::new(move || {
        App::with_state(AppState{executor: addr.clone()})
            // enable logger
   //         .middleware(middleware::Logger::default())
            .resource("/graphql", |r| r.method(http::Method::POST).with2(graphql))
            .resource("/graphql", |r| r.method(http::Method::GET).with2(graphql))
            .resource("/graphiql", |r| r.method(http::Method::GET).h(graphiql))
            .default_resource(|r| r.get().f(|_| HttpResponse::Found().header("location", "/graphiql").finish()))
    }).workers(num_cpus::get() * 2)
        .bind(&url)
        .expect("Failed to start server")
        .start();

    println!("Started http server: http://{}", url);
    let _ = sys.run();
}