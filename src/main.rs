use std::{str, fmt::Debug};
use hex::FromHex;
use rand::Rng;
use serde::Deserialize;
use sha1::{Sha1, Digest};
use tokio::{net::TcpStream, io::{AsyncReadExt, AsyncWriteExt}, time::Instant, task::JoinHandle};

#[derive(Clone, Deserialize)]
struct MinerConfiguration {
    username: String,
    mining_key: String,
    difficulty: String,
    rig_identifier: String,
    thread_count: u32
}

fn source_error<T, E: Debug>(reason: &str, err: E) -> Result<T, String> {
    Err(format!("{}\n\tbecause:\n{:?}", reason, err))
}

fn because_source_error<E: Debug>(reason: &str) -> impl Fn(E) -> String + '_ {
    move |err| format!("{}\n\tbecause:\n{:?}", reason, err)
}

fn because(reason: &str) -> impl Fn(String) -> String + '_ {
    move |original| format!("{}\n\tbecause:\n{}", reason, original)
}

fn format_hashrate(hashrate: f32) -> String {
    match hashrate {
        hr if hr >= 1e9 => format!("{:.2}GH/s", hr / 1e9),
        hr if hr >= 1e6 => format!("{:.2}MH/s", hr / 1e6),
        hr if hr >= 1e3 => format!("{:.2}kH/s", hr / 1e3),
        hr => format!("{:.2}H/s", hr)
    }
}

async fn read_string_from_socket(socket: &mut TcpStream) -> Result<String, String> {
    let mut buf = [0u8; 1024];
    let read = socket.read(&mut buf).await;
    match read {
        Ok(len) => {
            match String::from_utf8(buf[0..len].to_vec()) {
                Ok(value) => {
                    match value.strip_suffix('\n') {
                        Some(value) => Ok(value.to_string()),
                        None => Ok(value.to_string())
                    }
                    // println!("<<< {}", value);
                },
                Err(err) => source_error("Could not decode string from socket", err)
            }
        },
        Err(err) => source_error("Could not read from socket", err)
    }
}

async fn write_string_to_socket(socket: &mut TcpStream, value: String) -> Result<(), String> {
    // println!(">>> {}", value);
    match socket.write(value.as_bytes()).await {
        Ok(_) => Ok(()),
        Err(_) => Err("Could not write to socket".to_string())
    }
}

struct Connection {
    version: String,
    socket: TcpStream
}

impl Connection {
    pub async fn new(addr: &str) -> Result<Connection, String> {
        let mut socket = TcpStream::connect(addr).await
            .map_err(because_source_error("Could not connect"))?;
        
        let version = read_string_from_socket(&mut socket).await
            .map_err(because("Could not receive version"))?;
        
        Ok(Connection { version: version.into(), socket })
    }

    pub async fn request_job(&mut self, username: &str, mining_key: &str, difficulty: &str) -> Result<Job, String> {
        write_string_to_socket(&mut self.socket, format!("JOB,{},{},{}", username, difficulty, mining_key)).await
            .map_err(because("Could not send job request"))?;

        let job = read_string_from_socket(&mut self.socket).await
            .map_err(because("Could not receive job"))?;
        
        let data: Vec<&str> = job.split(',').collect();
        let base_hash = data[0].to_string();
        let target_hash = data[1].to_string();

        let difficulty = data[2].parse::<u32>()
            .map_err(because_source_error("Could not parse difficulty"))?;

        Ok(Job{
            base_hash,
            target_hash,
            difficulty
        })
    }

    pub async fn report_job(&mut self, nonce: u32, hashrate: u32, software_name: &str, rig_name: &str, multithread_id: &str) -> Result<JobFeedback, String> {
        write_string_to_socket(&mut self.socket, format!("{},{},{},{},,{}", nonce, hashrate, software_name, rig_name, multithread_id)).await
            .map_err(because("Could not send job report"))?;

        let feedback = read_string_from_socket(&mut self.socket).await
            .map_err(because("Could not receive feedback"))?;

        if feedback == "GOOD" || feedback == "BLOCK" {
            return Ok(JobFeedback::Good);
        }

        if feedback.starts_with("BAD,") {
            return Ok(JobFeedback::Bad(feedback[4..].to_string()));
        }
        
        Err(format!("Could not parse feedback: {}", feedback))
    }

    // pub async fn close(&mut self) -> Result<(), std::io::Error> {
    //     self.socket.shutdown().await
    // }
}

// #[derive(Clone)]
struct Job {
    base_hash: String,
    target_hash: String,
    difficulty: u32
}

#[derive(Clone, Copy)]
struct Solution {
    nonce: u32,
    elapsed_us: u128
}

enum JobFeedback {
    Good,
    Bad(String)
}

#[derive(Deserialize)]
struct PoolInfo {
    ip: String,
    port: u16
}

async fn get_server_address() -> Result<String, String> {
    let pool_info = reqwest::get("https://server.duinocoin.com/getPool").await
        .map_err(because_source_error("Could not get receive pool info"))?
        .json::<PoolInfo>().await
        .map_err(because_source_error("Could not deserialize pool info"))?;
    
    Ok(format!("{}:{}", pool_info.ip, pool_info.port))
}

fn solve(job: Job) -> Option<Solution> {
    let target = Vec::from_hex(job.target_hash).unwrap();

    let time_hash = Instant::now();

    let sha_base = Sha1::new_with_prefix(job.base_hash);

    for nonce in 0..(job.difficulty * 100 + 1) {
        let mut sha_temp = sha_base.clone();
        sha_temp.update(nonce.to_string());

        let hash = sha_temp.finalize().to_vec();

        if hash == target {
            let elapsed_us = time_hash.elapsed().as_micros();

            return Some(Solution { nonce, elapsed_us });
        }
    }

    None
}

async fn worker(configuration: MinerConfiguration, index: u32, multithread_id: &str) -> Result<(), String> {
    let addr = get_server_address().await?;

    println!("[worker{}] Server address is {}", index, addr);

    let mut connection = Connection::new(&addr).await
        .map_err(because("Could not create connection"))?;

    println!("[worker{}] Connected to server (version={})", index, connection.version);

    let mut time_work = Instant::now();
    let mut time_spent_mining = 0u128;
    let mut time_spent_in_connection = 0u128;
    let mut accepted_shares = 0;

    loop {
        let t = Instant::now();

        let job = connection.request_job(
                &configuration.username,
                &configuration.mining_key,
                &configuration.difficulty).await
            .map_err(because("Could not request job"))?;
        
        time_spent_in_connection += t.elapsed().as_micros();

        let worker_id = format!("worker{}", index).clone();
        let worker_id = worker_id.as_str();

        match solve(job) {
            Some(Solution{ nonce, elapsed_us }) => {
                let hashrate = 1e6 * nonce as f32 / elapsed_us as f32;

                let t = Instant::now();

                let feedback =
                    connection.report_job(
                        nonce,
                        hashrate as u32,
                        format!("Rust Duino Miner {}", clap::crate_version!()).as_str(),
                        &configuration.rig_identifier,
                        multithread_id).await
                    .map_err(because("Could not report job"))?;
                
                time_spent_in_connection += t.elapsed().as_micros();

                match feedback {
                    JobFeedback::Good => {
                        time_spent_mining += elapsed_us;
                        accepted_shares += 1;
                        println!("[{}] Share accepted ({}ms, {})",worker_id, elapsed_us / 1000, format_hashrate(hashrate));
                    },
                    JobFeedback::Bad(reason) => {
                        println!("[{}] Share rejected because: {}", worker_id, reason);
                    }
                }
            },
            None => {
                println!("[{}] Could not solve", worker_id);
            }
        }

        if accepted_shares % 10 == 0 {
            let work_time_share = time_spent_mining as f32 / time_work.elapsed().as_micros() as f32;
            let connection_time_share = time_spent_in_connection as f32 / time_work.elapsed().as_micros() as f32;
            println!(
                "[{}] Hash uptime: {:.2}% | Connection downtime: {:.2}%",
                worker_id,
                100.0 * work_time_share,
                100.0 * connection_time_share);
            
            time_work = Instant::now();
            time_spent_mining = 0;
            time_spent_in_connection = 0;
        }
    }
}

async fn root() -> Result<(), String> {
    let config_file = std::fs::File::open("duino-miner.yml")
        .map_err(because_source_error("Could not open configuration file"))?;
    
    let configuration: MinerConfiguration = serde_yaml::from_reader(config_file)
        .map_err(because_source_error("Could not deserialize configuration"))?;
    
    let multithread_id: u32 = rand::thread_rng().gen_range(10_000..100_000);

    let handles: Vec<JoinHandle<()>> = (0..configuration.thread_count).map(|i| {
        let multithread_id = format!("{}", multithread_id);
        let configuration = configuration.clone();

        tokio::spawn(async move {
            loop {
                let configuration = configuration.clone();
                let result = worker(configuration, i, &multithread_id).await;

                match result {
                    Ok(_) => return (),
                    Err(err) => println!("[worker{}] Error in worker:\n{}", i, err)
                }
            }
        })
    }).collect();

    for handle in handles {
        handle.await.unwrap();
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), String> {
    if let Err(err) = root().await {
        println!("[root] Error in root:\n{}", err);
    }

    Ok(())
}
