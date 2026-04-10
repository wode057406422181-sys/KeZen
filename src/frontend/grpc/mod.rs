pub mod server;
pub mod client;

pub use server::start_grpc_server;

pub(crate) mod kezen_proto {
    tonic::include_proto!("kezen");
}
