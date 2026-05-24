pub mod java;
pub mod javascript;
pub mod rust;

pub use java::resolve_java_dependency;
pub use javascript::{resolve_js_dependency, resolve_js_dependency_with_aliases};
pub use rust::resolve_rust_dependency;
