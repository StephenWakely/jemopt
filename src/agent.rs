use bollard::{
    container::{Config, CreateContainerOptions, LogOutput, StartContainerOptions},
    exec::{CreateExecOptions, StartExecResults},
    service::{HostConfig, PortBinding},
    Docker,
};
use futures::StreamExt;
use rand::{distributions::Alphanumeric, Rng};
use regex::Regex;
use std::{
    collections::HashMap,
    env,
    sync::atomic::{AtomicU16, Ordering},
    time::Duration,
};

use crate::dogstatsd;

#[derive(Debug, Clone)]
pub struct MallocConf {
    pub narenas: usize,
    pub tchache_max: usize,
    pub oversize_threshold: usize,
    pub dss: &'static str,
    pub background_thread: bool,
    pub muzzy_decay_ms: usize,
    pub lg_extent_max_active_fit: usize,
}

impl From<&[usize]> for MallocConf {
    fn from(value: &[usize]) -> Self {
        MallocConf {
            tchache_max: value[0] * 6_500,
            oversize_threshold: value[1] * 6_500,
            narenas: value[2],
            dss: match value[3] {
                0..3 => "disabled",
                3..7 => "primary",
                _ => "secondary",
            },
            background_thread: value[4] > 5,
            muzzy_decay_ms: value[5] * 100,
            lg_extent_max_active_fit: value[6],
        }
    }
}

impl ToString for MallocConf {
    fn to_string(&self) -> String {
        format!(
            r#"background_thread:{},narenas:{},tcache:false,dirty_decay_ms:0,muzzy_decay_ms:{},tcache_max:{},oversize_threshold:{},dss:{},lg_extent_max_active_fit:{}"#,
            if self.background_thread {
                "true"
            } else {
                "false"
            },
            self.narenas,
            self.muzzy_decay_ms,
            self.tchache_max,
            self.oversize_threshold,
            self.dss,
            self.lg_extent_max_active_fit,
        )
    }
}

/// Generate a random container name.
fn get_name() -> String {
    let mut rng = rand::thread_rng();
    format!(
        "groovin-{}",
        (0..10)
            .map(|_| rng.sample(Alphanumeric) as char)
            .collect::<String>()
    )
}

static PORT: AtomicU16 = AtomicU16::new(12500);

pub async fn run_container(
    conf: MallocConf,
    seconds: u64,
    payloads: bool,
    config: Option<&str>,
) -> Option<MemoryStats> {
    run_container_with_conf_string(&conf.to_string(), seconds, payloads, config).await
}

pub async fn run_container_with_conf_string(
    conf: &str,
    seconds: u64,
    payloads: bool,
    config: Option<&str>,
) -> Option<MemoryStats> {
    let config = config.map(|c| {
        env::current_dir()
            .map(|cwd| cwd.join(c))
            .expect("can get absolute path")
    });

    let conf = if conf == "" {
        String::new()
    } else {
        format!("MALLOC_CONF={conf}")
    };
    let docker = Docker::connect_with_socket_defaults().unwrap();
    let name = get_name();

    let port = PORT.fetch_add(1, Ordering::Relaxed);
    if port > 12700 {
        // 200 should be enough..
        PORT.store(12500, Ordering::Relaxed);
    }

    let mut env = vec!["DD_SITE=datad0g.com", "DD_API_KEY=00001"];

    if conf != "" {
        env.push(r#"LD_PRELOAD=/opt/lib/nosys.so:/opt/datadog-agent/embedded/lib/libjemalloc.so"#);
        env.push(&conf);
    }

    let mut volumes = vec!["/var/run/docker.sock:/var/run/docker.sock:ro".to_string()];

    if let Some(conf) = config {
        volumes.push(format!(
            "{conf}:/etc/datadog-agent/datadog.yaml",
            conf = conf.display()
        ));
    }

    docker
        .create_container(
            Some(CreateContainerOptions {
                name: &name,
                platform: None,
            }),
            Config {
                hostname: Some("zogglebork"),
                image: Some("datadog/agent-dev:nightly-main-8ea4e935-py3"),
                exposed_ports: Some({
                    let mut ports = HashMap::new();
                    ports.insert("8125/udp", HashMap::new());
                    ports
                }),
                host_config: Some(HostConfig {
                    //network_mode: Some("bridge".to_string()),
                    network_mode: Some("zorknet".to_string()),
                    binds: Some(volumes),
                    port_bindings: Some({
                        let mut bindings = HashMap::new();
                        bindings.insert(
                            "8125/udp".to_string(),
                            Some(vec![PortBinding {
                                host_ip: Some("127.0.0.1".to_string()),
                                host_port: Some(port.to_string()),
                            }]),
                        );
                        bindings
                    }),
                    nano_cpus: Some(2_000_000_000), // 2 cpus
                    auto_remove: Some(true),
                    ..Default::default()
                }),
                env: Some(env),
                network_disabled: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    docker
        .start_container(&name, None::<StartContainerOptions<String>>)
        .await
        .unwrap();

    println!("Container {name} port {port} running with {:?}", conf);

    if payloads {
        dogstatsd::spam(port, Duration::from_secs(seconds)).await;
    } else {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
    }

    let memory = get_memory(&docker, &name).await;

    match &memory {
        Some(memory) => println!("Agent {name} memory {} \x1b[31m{:?}\x1b[0m", conf, memory),
        None => println!("Failed to get memory"),
    }

    docker.stop_container(&name, None).await.unwrap();

    memory
}

#[derive(Debug, Clone)]
pub struct MemoryStats {
    agent: usize,
    process_agent: usize,
    security_agent: usize,
    trace_agent: usize,
}

impl MemoryStats {
    fn new(
        agent: usize,
        process_agent: usize,
        security_agent: usize,
        trace_agent: usize,
    ) -> Option<Self> {
        if agent > 0 && process_agent > 0 && security_agent > 0 && trace_agent > 0 {
            Some(MemoryStats {
                agent,
                process_agent,
                security_agent,
                trace_agent,
            })
        } else {
            None
        }
    }

    pub fn total(&self) -> usize {
        self.agent + self.process_agent + self.security_agent + self.trace_agent
    }
}

async fn get_memory(docker: &Docker, name: &str) -> Option<MemoryStats> {
    let ps = docker
        .create_exec(
            name,
            CreateExecOptions {
                attach_stdout: Some(true),
                cmd: Some(vec!["ps", "-aeo", "cmd,rss", "--no-headers"]), // | grep agent"]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let exec = docker.start_exec(&ps.id, None).await.unwrap();

    let StartExecResults::Attached { mut output, .. } = exec else {
        panic!("detached exec")
    };

    let agent_regex = Regex::new(r#"agent run *(\d+)$"#).unwrap();
    let process_agent_regex = Regex::new(r#"process-agent .* (\d+)$"#).unwrap();
    let security_agent_regex = Regex::new(r#"security-agent .* (\d+)$"#).unwrap();
    let trace_agent_regex = Regex::new(r#"trace-agent .* (\d+)$"#).unwrap();

    let mut agent = 0;
    let mut process_agent = 0;
    let mut security_agent = 0;
    let mut trace_agent = 0;

    while let Some(Ok(o)) = output.next().await {
        let line = match o {
            LogOutput::StdErr { message }
            | LogOutput::StdOut { message }
            | LogOutput::StdIn { message }
            | LogOutput::Console { message } => message,
        };

        for l in String::from_utf8_lossy(&line).split("\n") {
            if let Some(a) = agent_regex.captures(l) {
                agent = a[1].parse::<usize>().expect("memory should be parsed");
            }

            if let Some(a) = process_agent_regex.captures(l) {
                process_agent = a[1].parse::<usize>().expect("memory should be parsed");
            }

            if let Some(a) = security_agent_regex.captures(l) {
                security_agent = a[1].parse::<usize>().expect("memory should be parsed");
            }

            if let Some(a) = trace_agent_regex.captures(l) {
                trace_agent = a[1].parse::<usize>().expect("memory should be parsed");
            }
        }
    }

    MemoryStats::new(agent, process_agent, security_agent, trace_agent)
}
