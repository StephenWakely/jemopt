use clap::{Parser, Subcommand};
use genetic_algorithm::strategy::evolve::prelude::*;

mod agent;
mod dogstatsd;

const RUN_FOR_SECONDS: u64 = 60;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the GA.
    Evolve,
    /// Interpret the gene results from the evolution.
    Interpret {
        /// genes in CSV
        #[arg(short, long)]
        genes: String,
    },
    /// Run the agent with the given conf
    Run {
        /// The jemalloc conf to use. If not specified does not run jemalloc
        #[arg(short, long, default_value_t = String::new())]
        jemalloc: String,

        /// Time in seconds to run for
        #[arg(short, long, default_value_t = RUN_FOR_SECONDS)]
        seconds: u64,

	/// Send payloads via dogstatsd while running
        #[arg(short, long)]
	payloads: bool
    },
}

fn main() {
    let cli = Args::parse();

    match cli.command {
        Commands::Evolve => evolution(),
        Commands::Interpret { genes } => interpret(genes),
        Commands::Run { jemalloc, seconds, payloads } => run(&jemalloc, seconds, payloads),
    }
}

fn run(conf: &str, seconds: u64, payloads: bool) {
    match tokio::runtime::Runtime::new()
            .unwrap()
        .block_on(agent::run_container_with_conf_string(conf, seconds, payloads)) {
            Some(rss) => println!("RSS: {rss}"),
            None => println!("Duff run"),
        }
}

/// Interpret the genes
fn interpret(genes: String) {
    let genes = genes
        .split(",")
        .map(|gene| gene.parse::<usize>().expect("gene should be a number"))
        .collect::<Vec<_>>();

    let conf = agent::MallocConf::from(genes.as_ref());
    println!("{}", conf.to_string());
}

/// Run the GA evolution to get the best options for jemalloc that
/// result in the lowest memory usage.
fn evolution() {
    let genotype = ListGenotype::builder()
        .with_genes_size(7)
        .with_allele_list((0..20).collect())
        .build()
        .unwrap();

    let mut evolve = Evolve::builder()
        .with_genotype(genotype)
        .with_target_population_size(20)
        .with_max_stale_generations(50)
        .with_fitness(MallocFitness)
        .with_par_fitness(true)
        .with_fitness_ordering(FitnessOrdering::Minimize)
        .with_target_fitness_score(0)
        .with_mutate(MutateSingleGene::new(0.2))
        .with_crossover(CrossoverClone::new())
        .with_select(SelectElite::new(0.9))
        .with_reporter(EvolveReporterSimple::new_with_flags(
            10, true, true, true, true, true,
        ))
        .build()
        .unwrap();

    evolve.call();

    if let Some((best_genes, fitness_score)) = evolve.best_genes_and_fitness_score() {
        println!("Best genes {:?}", best_genes);
        println!(
            "Best conf {}",
            agent::MallocConf::from(best_genes.as_ref()).to_string()
        );
        println!("Best score {:?}", fitness_score);
    } else {
        println!("Duff run");
    }
}

#[derive(Clone, Debug)]
struct MallocFitness;

impl Fitness for MallocFitness {
    type Genotype = ListGenotype<usize>;
    fn calculate_for_chromosome(
        &mut self,
        chromosome: &FitnessChromosome<Self>,
        _genotype: &Self::Genotype,
    ) -> Option<FitnessValue> {
        let var_name = agent::MallocConf::from(chromosome.genes.as_ref());
        let conf = var_name;
        let rss = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(agent::run_container(conf, RUN_FOR_SECONDS));

        rss.map(|r| r as isize)
    }
}
