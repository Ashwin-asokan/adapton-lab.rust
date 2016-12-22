#![feature(field_init_shorthand)]
#![feature(rustc_private)]
#![feature(custom_derive)]

use std::fmt::Debug;
//use std::hash::Hash;
use std::rc::Rc;
use std::path::Path;

extern crate serialize;
extern crate time;
extern crate csv;
extern crate rand;

#[macro_use]
extern crate adapton;

use adapton::macros::*;
use adapton::collections::*;
use adapton::engine::*;
use rand::{Rng, SeedableRng, StdRng};
use std::marker::PhantomData;

#[derive(Clone,Debug,RustcEncodeable)]
pub enum NominalStrategy {
  Regular,
  ByContent,
}
#[derive(Clone,Debug,RustcEncodeable)]
pub struct GenerateParams {
  pub size: usize, 
  pub gauge: usize, 
  pub nominal_strategy:NominalStrategy
}

pub trait Generate<T> {
  fn generate<R:Rng>(rng:&mut R, params:&GenerateParams) -> T;
} 

pub trait Edit<T> : Clone {
  fn edit<R:Rng>(state:T, rng:&mut R, params:&GenerateParams) -> T;
}

pub trait Compute<Input,Output> {
  fn compute(Input) -> Output;
}

pub struct Computer<Input,Output,
                    Computer:Compute<Input,Output>> {
  pub computer: Computer,
  input:        PhantomData<Input>,
  output:       PhantomData<Output>
}

pub struct TestComputer<Input,Output,
                        InputDist:Generate<Input>+Edit<Input>,
                        Computer:Compute<Input,Output>> {
  identity:  Name,
  computer:  PhantomData<Computer>,
  input:     PhantomData<Input>,
  inputdist: PhantomData<InputDist>,
  output:    PhantomData<Output>
}

#[derive(Clone,Debug,RustcEncodeable)]
pub struct LabExpParams {
  pub sample_params: SampleParams,
  // TODO: Pretty-print input and output structures; graphmovie dump of experiment
  /// Number of change-batches to perform in a loop; each is interposed with computing the new output.
  pub change_batch_loopc: usize,
}

#[derive(Clone,Debug,RustcEncodeable)]
pub struct SampleParams {
  /// We convert this seed into a random-number-generator before generating and editing.
  pub input_seeds:       Vec<usize>, 
  /// Other parameters for generating the input.
  pub generate_params:   GenerateParams, 
  /// Whether to validate the output after each computation using the naive and DCG engines
  pub validate_output:   bool,
  /// Size of each batch of changes.
  pub change_batch_size: usize,
}

#[derive(Clone,Debug,RustcEncodeable)]
pub struct LabExpResults {
  pub samples: Vec<Sample>
}

/// The experiment consists of a loop over samples.  For each sample,
/// we switch back and forth between using the Naive engine, and using
/// the DCG engine.  We want to interleave this way for each sample in
/// order to compare outputs and metrics (counts and timings) on a
/// fine-grained scale.
#[derive(Clone,Debug,RustcEncodeable)]
pub struct Sample {
  pub params:       SampleParams,
  pub batch_name:   usize,   // Index/name the change batches; one sample per compute + change batch
  pub dcg_sample:   EngineSample,
  pub naive_sample: EngineSample,
  pub output_valid: Option<bool>
}

#[derive(Clone,Debug,RustcEncodeable)]
pub struct EngineSample {
  pub generate_input:   EngineMetrics,
  pub compute_output:   EngineMetrics,
  pub batch_edit_input: EngineMetrics,
}

#[derive(Clone,Debug,RustcEncodeable)]
pub struct EngineMetrics {
  pub time_ns:    u64,
  pub engine_cnt: Cnt,
}


pub trait SampleGen {
  fn sample(self:&mut Self) -> Option<Sample>;
}

pub struct TestEngineState<Input,Output,
                           InputDist:Generate<Input>+Edit<Input>,
                           Computer:Compute<Input,Output>> {
  pub engine:   Engine,
  pub input:    Input,
  inputdist:    PhantomData<InputDist>,
  computer:     PhantomData<Computer>,
  output:       PhantomData<Output>,
}

pub struct TestState<R:Rng+Clone,
                     Input,Output,
                     InputDist:Generate<Input>+Edit<Input>,
                     Computer:Compute<Input,Output>> {
  pub params:           LabExpParams,
  pub rng:              Box<R>,
  pub change_batch_num: usize,
  pub dcg_state:   TestEngineState<Input,Output,InputDist,Computer>,
  pub naive_state: TestEngineState<Input,Output,InputDist,Computer>,
  pub samples:     Vec<Sample>,
}

      
fn get_engine_metrics<X,F:FnOnce() -> X> (thunk:F) -> (X,EngineMetrics)
{
  let time_start = time::precise_time_ns();
  let (x,cnt) = cnt(thunk);
  let time_end = time::precise_time_ns();
  return (x, EngineMetrics{
    time_ns:time_end - time_start,
    engine_cnt:cnt,
  })
}

fn get_engine_sample<R:Rng+Clone,Input:Clone,Output,InputDist:Generate<Input>+Edit<Input>,Computer:Compute<Input,Output>> 
  (rng:&mut R, params:&SampleParams, input:Option<Input>) -> (Output,Input,EngineSample) 
{
  let mut rng2 = rng.clone();
  let (input, generate_input) : (Input,EngineMetrics) = match input {
    None        => get_engine_metrics(move || InputDist::generate(&mut rng2, &params.generate_params) ),
    Some(input) => get_engine_metrics(move || { input } )
  };
  let input2 = input.clone();
  let (output, compute_output): (Output,EngineMetrics) 
    = get_engine_metrics(move || Computer::compute(input2) );        
  let (input3, batch_edit_input): (_, EngineMetrics)   
    = get_engine_metrics(move || InputDist::edit(input, rng, &params.generate_params) );
  let engine_sample = EngineSample{
    generate_input,
    compute_output,
    batch_edit_input,
  };
  println!("{:?}", engine_sample); // XXX Temp
  return (output, input3, engine_sample)
}

fn get_sample_gen
  <Input:Clone,
   Output:Eq,
   InputDist:Generate<Input>+Edit<Input>,
   Computer:Compute<Input,Output>> 
  (params:&LabExpParams) 
   -> TestState<rand::StdRng,
                Input,Output,InputDist,Computer> 
{
  let mut rng1 = SeedableRng::from_seed(params.sample_params.input_seeds.as_slice());
  let mut rng2 = SeedableRng::from_seed(params.sample_params.input_seeds.as_slice());

  // Run Naive version.
  init_naive(); assert!(engine_is_naive());    
  let (naive_output, naive_input, naive_sample) = 
    get_engine_sample::<rand::StdRng,Input,Output,InputDist,Computer>
    (&mut rng1, &params.sample_params, None);

  // Save Rng1 in TestState, to restore before next sample.
  let rng_box = Box::new(rng1.clone());
    
  // Run DCG version.
  let _ = init_dcg(); assert!(engine_is_dcg());
  let (dcg_output, dcg_input, dcg_sample) = 
    get_engine_sample::<rand::StdRng,Input,Output,InputDist,Computer>
    (&mut rng2, &params.sample_params, None);
  
  // Compare outputs
  let output_valid = { if params.sample_params.validate_output 
                       { Some( naive_output == dcg_output ) }
                       else { None }};
  let sample = Sample{
    params:params.sample_params.clone(),
    batch_name:0, // Index/name the change batches; one sample per compute + change batch
    dcg_sample,
    naive_sample,
    output_valid,
  };
  let dcg = use_engine(Engine::Naive); // TODO-Minor: Rename this operation: "engine_swap" or something 
  TestState{      
    params:params.clone(),
    rng:rng_box, // save updated Rng for next sample
    dcg_state:TestEngineState{
      input: dcg_input, // save edited input
      engine: dcg, // save latest DCG
      output: PhantomData, inputdist: PhantomData, computer: PhantomData,      
    },
    naive_state:TestEngineState{
      input: naive_input, // save edited input
      engine: Engine::Naive, // A constant
      output: PhantomData, inputdist: PhantomData, computer: PhantomData,
    },
    change_batch_num: 1,
    samples:vec![sample],
  }
}

impl<Input:Clone,Output:Eq,
     InputDist:Generate<Input>+Edit<Input>,
     Computer:Compute<Input,Output>>
  SampleGen for TestState<rand::StdRng,Input,Output,InputDist,Computer> {
    fn sample (self:&mut Self) -> Option<Sample> {
      if ( self.change_batch_num == self.params.change_batch_loopc ) { None } else { 

        // Run Naive Version
        let _ = use_engine(Engine::Naive);
        assert!(engine_is_naive());
        let mut rng = self.rng.clone();
        let (naive_output, naive_input, naive_sample) = 
          get_engine_sample::<rand::StdRng,Input,Output,InputDist,Computer>
          (&mut rng, &self.params.sample_params, None);
        self.naive_state.input = naive_input;

        // Run DCG Version
        let dcg = self.dcg_state.engine.clone(); // Not sure about whether this Clone will do what we want; XXX
        let _ = use_engine(dcg);
        assert!(engine_is_dcg());
        let mut rng = self.rng.clone();
        let (dcg_output, dcg_input, dcg_sample) = 
          get_engine_sample::<rand::StdRng,Input,Output,InputDist,Computer>
          (&mut rng, &self.params.sample_params, None);
        self.dcg_state.engine = use_engine(Engine::Naive); // Swap out the DCG
        self.dcg_state.input = dcg_input;
        
        // Save the Rng for the next sample.
        self.rng = Box::new(*rng.clone());

        // Compare the two outputs for equality
        let output_valid = if self.params.sample_params.validate_output { 
          Some ( dcg_output == naive_output )
        } else { None } ;

        let sample = Sample{
          params:self.params.sample_params.clone(),
          batch_name:self.change_batch_num + 1,
          dcg_sample,
          naive_sample,
          output_valid,
        };
        self.change_batch_num += 1;
        Some(sample)
      }
    }
  }

// Lab experiment; Hides the Input, Output and Compute types, abstracting over them:
pub trait LabExp {
  fn name(self:&Self) -> Name;
  fn run(self:&Self, params:&LabExpParams) -> LabExpResults;
}

impl<Input:Clone,Output:Eq,
     InputDist:'static+Generate<Input>+Edit<Input>,
     Computer:'static+Compute<Input,Output>>
  LabExp for TestComputer<Input,Output,InputDist,Computer> {
    fn name(self:&Self) -> Name { self.identity.clone() }
    fn run(self:&Self, params:&LabExpParams) -> LabExpResults 
    {            
      let mut st = get_sample_gen::<Input,Output,InputDist,Computer>(params);
      loop {
        let sample = (&mut st).sample();
        match sample {
          Some(_) => continue,
          None => break,
        }
      };
      return LabExpResults {
        samples: st.samples,
      }
    }
  }


fn forkboilerplate () {
  use std::thread;
  let child =
    thread::Builder::new().stack_size(64 * 1024 * 1024).spawn(move || { 
      panic!("TODO");
    });
  let _ = child.unwrap().join();
}
  

fn csv_of_runtimes(path:&str, samples: Vec<Sample>) {
  let path = Path::new(path);
  let mut writer = csv::Writer::from_file(path).unwrap();
  for r in samples.into_iter() {
    //println!("{:?}",r);
    //writer.encode(r).ok().expect("CSV writer error");
  }
}

#[derive(Clone,Debug)]
pub struct ListInt_Uniform_Prepend<T> { T:PhantomData<T> }
#[derive(Clone,Debug)]
pub struct ListPt2D_Uniform_Prepend<T> { T:PhantomData<T> }

#[derive(Clone,Debug)]
pub struct ListInt_LazyMap { }
#[derive(Clone,Debug)]
pub struct ListInt_EagerMap { }
#[derive(Clone,Debug)]
pub struct ListInt_LazyFilter { }
#[derive(Clone,Debug)]
pub struct ListInt_EagerFilter { }
#[derive(Clone,Debug)]
pub struct ListInt_Reverse { }
#[derive(Clone,Debug)]
pub struct ListInt_LazyMergesort { }
#[derive(Clone,Debug)]
pub struct ListInt_EagerMergesort { }
#[derive(Clone,Debug)]
pub struct ListPt2D_Quickhull { }

impl Generate<List<usize>> for ListInt_Uniform_Prepend<List<usize>> {
  fn generate<R:Rng>(rng:&mut R, params:&GenerateParams) -> List<usize> {
    panic!("TODO")
  }
}

impl Edit<List<usize>> for ListInt_Uniform_Prepend<List<usize>> {
  fn edit<R:Rng>(state:List<usize>, rng:&mut R, params:&GenerateParams) -> List<usize> {
    panic!("TODO")
  }
}

impl Compute<List<usize>,List<usize>> for ListInt_EagerMap {
  fn compute(inp:List<usize>) -> List<usize> {
    panic!("TODO")
  }
}

impl Compute<List<usize>,List<usize>> for ListInt_EagerFilter {
  fn compute(inp:List<usize>) -> List<usize> {
    panic!("TODO")
  }
}

impl Compute<List<usize>,List<usize>> for ListInt_LazyMap {
  fn compute(inp:List<usize>) -> List<usize> {
    panic!("TODO")
  }
}

impl Compute<List<usize>,List<usize>> for ListInt_LazyFilter {
  fn compute(inp:List<usize>) -> List<usize> {
    panic!("TODO")
  }
}

impl Compute<List<usize>,List<usize>> for ListInt_Reverse {
  fn compute(inp:List<usize>) -> List<usize> {
    panic!("TODO")
  }
}

impl Compute<List<usize>,List<usize>> for ListInt_LazyMergesort {
  fn compute(inp:List<usize>) -> List<usize> {
    panic!("TODO")
  }
}

impl Compute<List<usize>,List<usize>> for ListInt_EagerMergesort {
  fn compute(inp:List<usize>) -> List<usize> {
    panic!("TODO")
  }
}

type Pt2D = (usize,usize); // TODO Fix this

impl Generate<List<Pt2D>> for ListPt2D_Uniform_Prepend<List<Pt2D>> { // TODO
  fn generate<R:Rng>(rng:&mut R, params:&GenerateParams) -> List<Pt2D> {
    panic!("TODO")
  }
}

impl Edit<List<Pt2D>> for ListPt2D_Uniform_Prepend<List<Pt2D>> { // TODO
  fn edit<R:Rng>(state:List<Pt2D>, rng:&mut R, params:&GenerateParams) -> List<Pt2D> {
    panic!("TODO")
  }
}

impl Compute<List<Pt2D>,List<Pt2D>> for ListPt2D_Quickhull {
  fn compute(inp:List<Pt2D>) -> List<Pt2D> {
    panic!("TODO")
  }
}

#[macro_export]
macro_rules! testcomputer {
  ( $name:expr, $inp:ty, $out:ty, $dist:ty, $comp:ty ) => {{ 
    Box::new( 
      TestComputer
        ::<$inp,$out,$dist,$comp>
      { 
        identity:$name,
        input:PhantomData, output:PhantomData, inputdist:PhantomData, computer:PhantomData
      }) 
  }}
}


/// This is the master list of all tests in the current Adapton Lab
pub fn all_tests() -> Vec<Box<LabExp>> {
  return vec![
    testcomputer!(name_of_str("eager-map"),
                  List<usize>,
                  List<usize>,
                  ListInt_Uniform_Prepend<List<usize>>,
                  ListInt_EagerMap)
      ,
    testcomputer!(name_of_str("eager-filter"),
                  List<usize>,
                  List<usize>,
                  ListInt_Uniform_Prepend<List<usize>>,
                  ListInt_EagerFilter)
      ,
    testcomputer!(name_of_str("lazy-map"),
                  List<usize>,
                  List<usize>,
                  ListInt_Uniform_Prepend<List<usize>>,
                  ListInt_LazyMap)
      ,
    testcomputer!(name_of_str("lazy-filter"),
                  List<usize>,
                  List<usize>,
                  ListInt_Uniform_Prepend<List<usize>>,
                  ListInt_LazyFilter)
      ,
    testcomputer!(name_of_str("reverse"),
                  List<usize>,
                  List<usize>,
                  ListInt_Uniform_Prepend<List<usize>>,
                  ListInt_Reverse)
      ,
    testcomputer!(name_of_str("eager-mergesort"),
                  List<usize>,
                  List<usize>,
                  ListInt_Uniform_Prepend<List<usize>>,
                  ListInt_EagerMergesort)
      ,
    testcomputer!(name_of_str("lazy-mergesort"),
                  List<usize>,
                  List<usize>,
                  ListInt_Uniform_Prepend<List<usize>>,
                  ListInt_EagerMergesort)
      ,
    testcomputer!(name_of_str("quickhull"),
                  List<Pt2D>,
                  List<Pt2D>,
                  ListPt2D_Uniform_Prepend<List<Pt2D>>,
                  ListPt2D_Quickhull)
      ,
  ]
}

fn labexp_params_defaults() -> LabExpParams {
  return LabExpParams {
    sample_params: SampleParams{
      input_seeds: vec![0],
      generate_params: GenerateParams{
        size:10,
        gauge:1,
        nominal_strategy:NominalStrategy::Regular,
      },
      validate_output: true,
      change_batch_size: 1,
    },
    change_batch_loopc:10,
  }
}

fn run_all_tests() {
  let params = labexp_params_defaults();
  let tests = all_tests();
  for test in tests.iter() {
    println!("Test: {:?}", test.name());
    let results = test.run(&params);
  }
}

#[test]
fn test_all() { run_all_tests() }
fn main() { run_all_tests() }
