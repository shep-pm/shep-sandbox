use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use shep_client::shep_core::{protocol::ProcessInfo, status::ProcStatus};

use serde_json::Value;
use tempfile::tempdir;

use super::{Applies, Args, Error, SKELETON, Taken, create, observe::Socket, parse, running};

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_owned()).collect()
}

fn run(dir: &Path, list: &[String]) -> Result<PathBuf, Error> {
    super::run(dir, &parse(list)?, None)
}

fn skeleton() -> Value {
    serde_json::from_str(SKELETON).expect("the skeleton is JSON")
}

#[test]
fn with_no_path_it_writes_sandbox_json_in_the_directory() {
    let dir = tempdir().unwrap();

    let written = run(dir.path(), &[]).unwrap();

    assert_eq!(written, dir.path().join("sandbox.json"));
    assert_eq!(fs::read_to_string(written).unwrap(), SKELETON);
}

#[test]
fn a_relative_path_is_read_against_the_directory() {
    let dir = tempdir().unwrap();

    let written = run(dir.path(), &args(&["web.json"])).unwrap();

    assert_eq!(written, dir.path().join("web.json"));
    assert_eq!(fs::read_to_string(written).unwrap(), SKELETON);
    assert!(!dir.path().join("sandbox.json").exists());
}

#[test]
fn an_absolute_path_is_written_where_it_says() {
    let dir = tempdir().unwrap();
    let elsewhere = tempdir().unwrap();
    let target = elsewhere.path().join("api.json");

    let written = run(dir.path(), &args(&[target.to_str().unwrap()])).unwrap();

    assert_eq!(written, target);
    assert_eq!(fs::read_to_string(target).unwrap(), SKELETON);
}

#[test]
fn a_file_that_exists_is_refused_and_left_as_it_was() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("sandbox.json");
    fs::write(&target, "mine").unwrap();

    let err = run(dir.path(), &[]).unwrap_err();

    assert!(
        matches!(&err, Error::Exists(path) if *path == target),
        "{err}"
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "mine");
}

#[test]
fn a_directory_that_does_not_exist_is_a_write_failure() {
    let dir = tempdir().unwrap();

    let err = run(dir.path(), &args(&["missing/sandbox.json"])).unwrap_err();

    assert!(matches!(err, Error::Write { .. }), "{err}");
}

#[test]
fn a_file_whose_write_fails_is_removed_so_a_retry_can_create_it() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("sandbox.json");

    let err = create(&target, |_| Err(std::io::Error::other("disk full"))).unwrap_err();

    assert!(matches!(err, Error::Write { .. }), "{err}");
    assert!(!target.exists());
    run(dir.path(), &[]).unwrap();
}

#[test]
fn a_second_path_or_a_flag_is_a_usage_error() {
    let dir = tempdir().unwrap();

    for list in [&["a.json", "b.json"][..], &["--force"][..]] {
        let err = run(dir.path(), &args(list)).unwrap_err();

        assert!(matches!(err, Error::Usage { .. }), "{list:?}: {err}");
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn the_skeleton_names_every_setting_crabpot_reads_as_a_list() {
    let skeleton = skeleton();

    for (group, key) in [
        ("filesystem", "allowWrite"),
        ("filesystem", "denyRead"),
        ("filesystem", "denyWrite"),
        ("network", "allowedDomains"),
        ("network", "deniedDomains"),
        ("network", "publish"),
        ("network", "allowTcp"),
    ] {
        assert!(skeleton[group][key].is_array(), "{group}.{key}");
    }
}

#[test]
fn the_skeleton_opens_the_cwd_for_writing_and_asks_for_nothing_else() {
    let skeleton = skeleton();

    assert_eq!(
        skeleton["filesystem"]["allowWrite"],
        serde_json::json!(["."])
    );
    for (group, key) in [
        ("filesystem", "denyRead"),
        ("filesystem", "denyWrite"),
        ("network", "allowedDomains"),
        ("network", "deniedDomains"),
        ("network", "publish"),
        ("network", "allowTcp"),
    ] {
        assert_eq!(skeleton[group][key], serde_json::json!([]), "{group}.{key}");
    }
}

#[test]
fn every_key_besides_the_settings_is_a_comment_crabpot_drops() {
    fn settings(object: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = object
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|key| !key.starts_with("//"))
            .collect();
        keys.sort_unstable();
        keys
    }
    let skeleton = skeleton();

    assert_eq!(settings(&skeleton), ["filesystem", "network"]);
    assert_eq!(
        settings(&skeleton["filesystem"]),
        ["allowWrite", "denyRead", "denyWrite"]
    );
    assert_eq!(
        settings(&skeleton["network"]),
        ["allowTcp", "allowedDomains", "deniedDomains", "publish"]
    );
}

mod linux {
    use std::net::SocketAddr;

    use super::super::observe::{Socket, linux};

    // Real /proc/net/tcp rows. Addresses are the kernel's native-endian words,
    // so 0100007F is 127.0.0.1 and the port is plain big-endian hex.
    const TCP: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 40001 1 0000000000000000 100 0 0 10 0
   1: 0F00000A:1F90 0900000A:C738 01 00000000:00000000 00:00000000 00000000  1000        0 40002 1 0000000000000000 20 4 30 10 -1
   2: 0F00000A:D2A4 0500000A:0CEA 01 00000000:00000000 00:00000000 00000000  1000        0 40003 1 0000000000000000 20 4 30 10 -1
   3: 0100007F:C351 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 40004 1 0000000000000000 20 4 30 10 -1
   4: 0100007F:1F90 0100007F:C351 01 00000000:00000000 00:00000000 00000000  1000        0 40005 1 0000000000000000 20 4 30 10 -1
   5: 0F00000A:D2A5 0500000A:01BB 06 00000000:00000000 03:00000000 00000000     0        0 40006 1 0000000000000000
   6: 0100007F:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000   101        0 99999 1 0000000000000000 100 0 0 10 0
";

    const TCP6: &str = "\
  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:20FB 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 40011 1 0000000000000000 100 0 0 10 0
   1: B80D0120000000000000000001000000:D2A4 B80D0120000000000000000005000000:1538 01 00000000:00000000 00:00000000 00000000  1000        0 40012 1 0000000000000000 20 4 30 10 -1
   2: 0000000000000000FFFF00000F00000A:D2A6 0000000000000000FFFF00000500000A:0CEA 01 00000000:00000000 00:00000000 00000000  1000        0 40013 1 0000000000000000 20 4 30 10 -1
";

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn held(inodes: &[u64]) -> std::collections::HashSet<u64> {
        inodes.iter().copied().collect()
    }

    #[test]
    fn a_listener_is_read_with_its_address_and_port() {
        let sockets = linux::sockets(TCP, false, &held(&[40001]));

        assert_eq!(sockets, [Socket::Listening(addr("0.0.0.0:8080"))]);
    }

    #[test]
    fn a_connection_gives_both_ends() {
        let sockets = linux::sockets(TCP, false, &held(&[40002, 40003]));

        assert_eq!(
            sockets,
            [
                Socket::Connected {
                    local: addr("10.0.0.15:8080"),
                    remote: addr("10.0.0.9:51000"),
                },
                Socket::Connected {
                    local: addr("10.0.0.15:53924"),
                    remote: addr("10.0.0.5:3306"),
                },
            ]
        );
    }

    #[test]
    fn both_ends_of_a_loopback_connection_are_read_from_their_rows() {
        let sockets = linux::sockets(TCP, false, &held(&[40004, 40005]));

        assert_eq!(
            sockets,
            [
                Socket::Connected {
                    local: addr("127.0.0.1:50001"),
                    remote: addr("127.0.0.1:8080"),
                },
                Socket::Connected {
                    local: addr("127.0.0.1:8080"),
                    remote: addr("127.0.0.1:50001"),
                },
            ]
        );
    }

    #[test]
    fn only_sockets_the_sheep_holds_are_read() {
        let sockets = linux::sockets(TCP, false, &held(&[40001]));

        assert_eq!(sockets.len(), 1, "inode 99999 belongs to someone else");
    }

    #[test]
    fn a_state_besides_listen_and_established_is_left_out() {
        let sockets = linux::sockets(TCP, false, &held(&[40006]));

        assert_eq!(sockets, []);
    }

    #[test]
    fn ipv6_addresses_are_read_word_by_word() {
        let sockets = linux::sockets(TCP6, true, &held(&[40011, 40012]));

        assert_eq!(
            sockets,
            [
                Socket::Listening(addr("[::]:8443")),
                Socket::Connected {
                    local: addr("[2001:db8::1]:53924"),
                    remote: addr("[2001:db8::5]:5432"),
                },
            ]
        );
    }

    #[test]
    fn an_ipv4_connection_on_a_dual_stack_socket_reads_as_ipv4() {
        let sockets = linux::sockets(TCP6, true, &held(&[40013]));

        assert_eq!(
            sockets,
            [Socket::Connected {
                local: addr("10.0.0.15:53926"),
                remote: addr("10.0.0.5:3306"),
            }]
        );
    }

    #[test]
    fn a_socket_inode_is_read_from_the_fd_link_target() {
        assert_eq!(linux::socket_inode("socket:[40001]"), Some(40001));
        assert_eq!(linux::socket_inode("/dev/null"), None);
        assert_eq!(linux::socket_inode("pipe:[40001]"), None);
        assert_eq!(linux::socket_inode("anon_inode:[eventpoll]"), None);
    }
}

mod classify {
    use std::net::SocketAddr;

    use super::super::observe::{Observed, Socket};

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn connected(local: &str, remote: &str) -> Socket {
        Socket::Connected {
            local: addr(local),
            remote: addr(remote),
        }
    }

    fn set(list: &[&str]) -> std::collections::BTreeSet<SocketAddr> {
        list.iter().map(|text| addr(text)).collect()
    }

    #[test]
    fn a_listener_is_published() {
        let observed = Observed::from_sockets(&[Socket::Listening(addr("0.0.0.0:8080"))]);

        assert_eq!(observed.listening, set(&["0.0.0.0:8080"]));
        assert!(observed.dials.is_empty());
    }

    #[test]
    fn an_accepted_connection_is_not_a_dial() {
        let observed = Observed::from_sockets(&[
            Socket::Listening(addr("0.0.0.0:8080")),
            connected("10.0.0.15:8080", "10.0.0.9:51000"),
        ]);

        assert!(observed.dials.is_empty(), "{observed:?}");
    }

    #[test]
    fn an_accepted_connection_is_not_a_dial_even_when_listed_first() {
        let observed = Observed::from_sockets(&[
            connected("10.0.0.15:8080", "10.0.0.9:51000"),
            Socket::Listening(addr("0.0.0.0:8080")),
        ]);

        assert!(observed.dials.is_empty(), "{observed:?}");
    }

    #[test]
    fn a_connection_from_an_ephemeral_port_is_a_dial() {
        let observed = Observed::from_sockets(&[connected("10.0.0.15:53924", "10.0.0.5:3306")]);

        assert_eq!(observed.dials, set(&["10.0.0.5:3306"]));
    }

    #[test]
    fn a_loopback_dial_to_its_own_listener_is_left_out() {
        let observed = Observed::from_sockets(&[
            Socket::Listening(addr("0.0.0.0:8080")),
            connected("127.0.0.1:50001", "127.0.0.1:8080"),
            connected("127.0.0.1:8080", "127.0.0.1:50001"),
        ]);

        assert!(observed.dials.is_empty(), "{observed:?}");
    }

    #[test]
    fn a_dual_stack_listener_is_dialed_over_either_loopback() {
        let observed = Observed::from_sockets(&[
            Socket::Listening(addr("[::]:8080")),
            connected("[::1]:50002", "[::1]:8080"),
            connected("127.0.0.1:50003", "127.0.0.1:8080"),
        ]);

        assert!(observed.dials.is_empty(), "{observed:?}");
    }

    #[test]
    fn a_loopback_dial_to_the_same_port_on_another_loopback_address_is_kept() {
        let observed = Observed::from_sockets(&[
            Socket::Listening(addr("127.0.0.1:6379")),
            connected("127.0.0.2:50001", "127.0.0.2:6379"),
        ]);

        assert_eq!(observed.dials, set(&["127.0.0.2:6379"]));
    }

    #[test]
    fn a_connection_on_another_address_at_a_listener_port_is_a_dial() {
        let observed = Observed::from_sockets(&[
            Socket::Listening(addr("127.0.0.1:6379")),
            connected("10.0.0.15:6379", "10.0.0.5:51000"),
        ]);

        assert_eq!(observed.dials, set(&["10.0.0.5:51000"]));
    }

    #[test]
    fn an_ipv4_wildcard_listener_does_not_serve_ipv6_loopback() {
        let observed = Observed::from_sockets(&[
            Socket::Listening(addr("0.0.0.0:8080")),
            connected("[::1]:50002", "[::1]:8080"),
        ]);

        assert_eq!(observed.dials, set(&["[::1]:8080"]));
    }

    #[test]
    fn a_loopback_dial_to_another_port_is_kept() {
        let observed = Observed::from_sockets(&[
            Socket::Listening(addr("0.0.0.0:8080")),
            connected("127.0.0.1:50001", "127.0.0.1:5432"),
        ]);

        assert_eq!(observed.dials, set(&["127.0.0.1:5432"]));
    }

    #[test]
    fn a_dial_to_the_listener_port_of_another_host_is_kept() {
        let observed = Observed::from_sockets(&[
            Socket::Listening(addr("0.0.0.0:8080")),
            connected("10.0.0.15:50001", "10.0.0.7:8080"),
        ]);

        assert_eq!(observed.dials, set(&["10.0.0.7:8080"]));
    }

    #[test]
    fn ipv6_listeners_and_dials_keep_their_family() {
        let observed = Observed::from_sockets(&[
            Socket::Listening(addr("[::]:8443")),
            connected("[2001:db8::1]:53924", "[2001:db8::5]:5432"),
        ]);

        assert_eq!(observed.listening, set(&["[::]:8443"]));
        assert_eq!(observed.dials, set(&["[2001:db8::5]:5432"]));
    }

    #[test]
    fn a_dial_to_an_https_port_is_kept_apart_from_the_others() {
        let observed = Observed::from_sockets(&[
            connected("10.0.0.15:53924", "203.0.113.7:443"),
            connected("10.0.0.15:53925", "10.0.0.5:3306"),
        ]);

        assert_eq!(observed.https, set(&["203.0.113.7:443"]));
        assert_eq!(observed.dials, set(&["10.0.0.5:3306"]));
    }

    #[test]
    fn the_same_dial_twice_is_listed_once() {
        let observed = Observed::from_sockets(&[
            connected("10.0.0.15:53924", "10.0.0.5:3306"),
            connected("10.0.0.15:53925", "10.0.0.5:3306"),
        ]);

        assert_eq!(observed.dials.len(), 1);
    }
}

mod macos {
    use std::net::SocketAddr;

    use super::super::observe::{Socket, macos};

    // `lsof -a -p <pids> -i -n -P` on macOS 15.
    const LSOF: &str = "\
COMMAND   PID USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME
node    41001  rin   23u  IPv4 0x6b1c2a5e7d3f4a10      0t0  TCP *:8080 (LISTEN)
node    41001  rin   24u  IPv4 0x6b1c2a5e7d3f4b20      0t0  TCP 10.0.0.15:8080->10.0.0.9:51000 (ESTABLISHED)
node    41001  rin   25u  IPv4 0x6b1c2a5e7d3f4c30      0t0  TCP 10.0.0.15:53924->10.0.0.5:3306 (ESTABLISHED)
node    41001  rin   26u  IPv4 0x6b1c2a5e7d3f4d40      0t0  TCP 127.0.0.1:50001->127.0.0.1:8080 (ESTABLISHED)
node    41001  rin   27u  IPv6 0x6b1c2a5e7d3f4e50      0t0  TCP *:8443 (LISTEN)
node    41001  rin   28u  IPv6 0x6b1c2a5e7d3f4f60      0t0  TCP [2001:db8::1]:53924->[2001:db8::5]:5432 (ESTABLISHED)
node    41001  rin   29u  IPv6 0x6b1c2a5e7d3f5070      0t0  TCP [::1]:9229 (LISTEN)
node    41001  rin   30u  IPv4 0x6b1c2a5e7d3f5180      0t0  UDP *:5353
node    41001  rin   31u  IPv4 0x6b1c2a5e7d3f5290      0t0  TCP 10.0.0.15:53930->10.0.0.5:3306 (CLOSE_WAIT)
helper  41002  rin   10u  IPv4 0x6b1c2a5e7d3f53a0      0t0  TCP 127.0.0.1:6379 (LISTEN)
";

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    fn listeners_get_the_wildcard_of_their_family() {
        let sockets = macos::parse(LSOF);

        assert!(sockets.contains(&Socket::Listening(addr("0.0.0.0:8080"))));
        assert!(sockets.contains(&Socket::Listening(addr("[::]:8443"))));
        assert!(sockets.contains(&Socket::Listening(addr("[::1]:9229"))));
        assert!(sockets.contains(&Socket::Listening(addr("127.0.0.1:6379"))));
    }

    #[test]
    fn an_established_connection_gives_both_ends() {
        let sockets = macos::parse(LSOF);

        assert!(sockets.contains(&Socket::Connected {
            local: addr("10.0.0.15:53924"),
            remote: addr("10.0.0.5:3306"),
        }));
        assert!(sockets.contains(&Socket::Connected {
            local: addr("[2001:db8::1]:53924"),
            remote: addr("[2001:db8::5]:5432"),
        }));
    }

    #[test]
    fn udp_a_closing_connection_and_the_header_are_left_out() {
        let sockets = macos::parse(LSOF);

        assert_eq!(sockets.len(), 8, "{sockets:?}");
    }

    #[test]
    fn an_ipv4_mapped_address_reads_as_ipv4() {
        let sockets = macos::parse(
            "node 41001 rin 28u IPv6 0x6b1c2a5e7d3f4f60 0t0 TCP \
             [::ffff:127.0.0.1]:50001->[::ffff:127.0.0.1]:8080 (ESTABLISHED)\n",
        );

        assert_eq!(
            sockets,
            [Socket::Connected {
                local: addr("127.0.0.1:50001"),
                remote: addr("127.0.0.1:8080"),
            }]
        );
    }

    #[test]
    fn exit_1_with_rows_is_the_sockets_of_the_pids_that_have_any() {
        // lsof exits 1 when any listed pid holds no internet socket.
        let sockets = macos::sockets_of(Some(1), LSOF, "").unwrap();

        assert_eq!(sockets, macos::parse(LSOF));
    }

    #[test]
    fn exit_0_is_the_sockets() {
        assert_eq!(macos::sockets_of(Some(0), LSOF, "").unwrap().len(), 8);
    }

    #[test]
    fn exit_1_with_no_rows_is_no_sockets() {
        assert_eq!(macos::sockets_of(Some(1), "", "").unwrap(), []);
    }

    #[test]
    fn any_other_exit_is_a_failure_that_says_why() {
        let err = macos::sockets_of(Some(2), "", "lsof: illegal option\n").unwrap_err();

        assert!(err.to_string().contains("illegal option"), "{err}");
    }

    #[test]
    fn a_signal_is_a_failure() {
        assert!(macos::sockets_of(None, "", "").is_err());
    }

    #[test]
    fn no_output_is_no_sockets() {
        assert_eq!(macos::parse(""), []);
    }
}

mod arguments {
    use super::{Args, Error, args, parse};

    impl Args {
        fn new(path: Option<&str>, from: Option<&str>) -> Self {
            Self {
                path: path.map(str::to_owned),
                from: from.map(str::to_owned),
            }
        }
    }

    #[test]
    fn from_takes_the_next_argument_as_the_sheep() {
        let parsed = parse(&args(&["--from", "web"])).unwrap();

        assert_eq!(parsed, Args::new(None, Some("web")));
    }

    #[test]
    fn a_path_and_from_come_in_either_order() {
        let before = parse(&args(&["web.json", "--from", "web"])).unwrap();
        let after = parse(&args(&["--from", "web", "web.json"])).unwrap();

        assert_eq!(before, Args::new(Some("web.json"), Some("web")));
        assert_eq!(before, after);
    }

    #[test]
    fn from_with_no_sheep_is_a_usage_error() {
        let err = parse(&args(&["web.json", "--from"])).unwrap_err();

        assert!(
            matches!(&err, Error::Usage { arg } if arg == "--from"),
            "{err}"
        );
    }

    #[test]
    fn from_twice_is_a_usage_error() {
        let err = parse(&args(&["--from", "web", "--from", "api"])).unwrap_err();

        assert!(
            matches!(&err, Error::Usage { arg } if arg == "--from"),
            "{err}"
        );
    }
}

mod lookup {
    use super::{Error, ProcStatus, ProcessInfo, running};

    fn sheep(id: u32, name: &str, status: ProcStatus, pid: Option<u32>) -> ProcessInfo {
        let mut info = ProcessInfo::builder(id, name, status).build();
        info.pid = pid;
        info
    }

    #[test]
    fn a_running_sheep_gives_its_row() {
        let flock = [
            sheep(1, "api", ProcStatus::Online, Some(100)),
            sheep(2, "web", ProcStatus::Online, Some(200)),
        ];

        let row = running("web", &flock).unwrap();

        assert_eq!((row.id, row.pid), (2, Some(200)));
    }

    #[test]
    fn a_sheep_that_is_not_running_is_refused_by_name() {
        let flock = [sheep(2, "web", ProcStatus::Stopped, None)];

        let err = running("web", &flock).unwrap_err();

        assert!(
            matches!(&err, Error::NotRunning { sheep } if sheep == "web"),
            "{err}"
        );
        assert_eq!(
            err.to_string(),
            "web is not running, so there are no sockets to read"
        );
    }

    #[test]
    fn a_sheep_waiting_to_restart_is_not_running() {
        let flock = [sheep(2, "web", ProcStatus::WaitingRestart, None)];

        let err = running("web", &flock).unwrap_err();

        assert!(matches!(err, Error::NotRunning { .. }), "{err}");
    }

    #[test]
    fn a_name_the_shepherd_does_not_hold_is_refused_by_name() {
        let flock = [sheep(1, "api", ProcStatus::Online, Some(100))];

        let err = running("web", &flock).unwrap_err();

        assert!(
            matches!(&err, Error::UnknownSheep { sheep } if sheep == "web"),
            "{err}"
        );
    }

    #[test]
    fn of_several_instances_the_running_one_is_taken() {
        let flock = [
            sheep(1, "web", ProcStatus::Stopped, None),
            sheep(2, "web", ProcStatus::Online, Some(200)),
        ];

        assert_eq!(running("web", &flock).unwrap().id, 2);
    }
}

mod exited {
    use super::{super::observe, Error};

    #[test]
    fn a_process_that_exited_before_its_sockets_were_read_is_not_running() {
        let err = Error::observed(observe::Error::Gone, "web");

        assert!(
            matches!(&err, Error::NotRunning { sheep } if sheep == "web"),
            "{err}"
        );
    }

    #[test]
    fn any_other_read_failure_stays_a_read_failure() {
        let err = Error::observed(observe::Error::Unsupported, "web");

        assert!(
            matches!(err, Error::Observe(observe::Error::Unsupported)),
            "{err}"
        );
    }
}

mod from_a_sheep;
