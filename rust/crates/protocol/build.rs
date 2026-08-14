use std::{env, path::PathBuf};

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let proto_root = manifest_dir.join("../../../proto");
    let proto_file = proto_root.join("dsh/remote/v1/remote.proto");

    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    println!("cargo:rustc-env=PROTOC={}", protoc.display());
    env::set_var("PROTOC", protoc);

    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&[proto_file], std::slice::from_ref(&proto_root))
        .expect("compile Protobuf contract");

    println!("cargo:rerun-if-changed={}", proto_root.display());
}
