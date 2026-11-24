//! The control-command registry. The front end serves it on a named pipe (`\.\pipe\nxrs_ctl_<process id>`, one
//! command line per connection, the reply text back; scripts/play_scenario.py is the client), and any part registers
//! the commands it answers: graphics its screenshot and pass dumps, the input layer its presses, and so on.

use std::collections::BTreeMap;
use std::sync::Mutex;

pub type Handler = Box<dyn Fn(&[&str]) -> String + Send + Sync>;

struct Cmd {
    help: &'static str,
    f: std::sync::Arc<Handler>,
}

static CMDS: Mutex<BTreeMap<String, Cmd>> = Mutex::new(BTreeMap::new());

/// Registers `name` (replacing an earlier one). `help` is one line: arguments and what it does.
pub fn register(
    name: &str,
    help: &'static str,
    f: impl Fn(&[&str]) -> String + Send + Sync + 'static,
) {
    CMDS.lock().unwrap().insert(
        name.to_string(),
        Cmd {
            help,
            f: std::sync::Arc::new(Box::new(f)),
        },
    );
}

/// Runs one command line ("name arg arg..."); unknown names and an empty line answer with the list.
pub fn run(line: &str) -> String {
    let args: Vec<&str> = line.split_whitespace().collect();
    let Some(&name) = args.first() else {
        return help();
    };
    let f = CMDS.lock().unwrap().get(name).map(|c| c.f.clone());
    match f {
        Some(f) => {
            let mut r = f(&args[1..]);
            if !r.ends_with('\n') {
                r.push('\n');
            }
            r
        }
        None if name == "help" => help(),
        None => format!("unknown command {name}\n{}", help()),
    }
}

pub fn help() -> String {
    let c = CMDS.lock().unwrap();
    let mut s = String::new();
    for (n, cmd) in c.iter() {
        s.push_str(&format!("{n} {}\n", cmd.help));
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn registry() {
        super::register("echo", "<words>: says them back", |a| a.join(" "));
        assert_eq!(super::run("echo a  b"), "a b\n");
        assert!(super::run("nope").starts_with("unknown command nope"));
        assert!(super::run("help").contains("echo <words>"));
    }
}
