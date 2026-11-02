//! Native argv oracle for shell qualification. Completion is forwarded to the
//! actual application; submitted arguments are recorded before real execution.
use std::{
    env, fs,
    process::{Command, exit},
};

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let backend = env::var_os("TWIGSMITH_ARGUMENT_BACKEND").expect("fixture backend");
    let completion = arguments.first().is_some_and(|arg| arg == "__complete");
    if !completion {
        let capture = env::var_os("TWIGSMITH_ARGUMENT_CAPTURE").expect("fixture capture");
        fs::write(capture, serde_json::to_vec(&arguments).unwrap()).unwrap();
    }
    let status = Command::new(backend).args(&arguments).status().unwrap();
    if !completion {
        println!("NATIVE_ARGUMENTS_RECORDED");
    }
    exit(status.code().unwrap_or(1));
}
