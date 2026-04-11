pub mod client;
pub mod server;

pub use server::start_grpc_server;

pub(crate) mod kezen_proto {
    tonic::include_proto!("kezen");
}
