/// Side effects returned by `update()` and executed by the runtime.
/// `update()` is synchronous — anything async is a Cmd.
#[derive(Debug)]
pub enum Cmd {
    None,
}
