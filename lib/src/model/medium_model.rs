use std::{
  collections::HashMap,
  convert::TryInto,
  fs::{self},
  iter::zip,
  path::Path,
};

use luminal::prelude::*;
use luminal_nn::{Linear, ReLU};
use luminal_training::{mse_loss, sgd_on_graph, Autograd};
use tracing::info;

use crate::scalar::copy_graph_roughly;

// const FILE_PATH: &str = "data/rp.data";

pub type InputsVec = Vec<[f32; 9]>;
pub type OutputsVec = Vec<f32>;

pub type Model = (Linear<9, 16>, ReLU, Linear<16, 16>, ReLU, Linear<16, 1>);

pub fn read_dataset(path: &Path) -> Result<(InputsVec, OutputsVec), std::io::Error> {
  let content: String = fs::read_to_string(path)?;
  Ok(parse_dataset(content))
}

pub fn parse_dataset(content: String) -> (InputsVec, OutputsVec) {
  let content: Vec<String> = content.lines().map(String::from).collect();

  // todo: why no csv?
  let mut x: InputsVec = Vec::new();
  let mut y: OutputsVec = Vec::new();
  for line in content {
    let mut parts: Vec<&str> = line.split(" ").collect();
    parts.retain(|&a| a != "");
    let parts: OutputsVec = parts.iter().map(|a| a.parse::<f32>().unwrap()).collect();
    let len = parts.len();
    x.push(parts[0..len - 1].try_into().unwrap());
    if parts[len - 1] == 2.0 {
      y.push(0.);
    } else {
      y.push(1.);
    }
  }
  (x, y)
}

pub fn split_dataset(
  x: InputsVec,
  y: OutputsVec,
  ratio: f32,
) -> (InputsVec, InputsVec, OutputsVec, OutputsVec) {
  let len = x.len();
  let len_short = (len as f32 * ratio) as usize;
  let x_train = x[0..len_short].to_vec();
  let x_test = x[len_short..len - 1].to_vec();
  let y_train = y[0..len_short].to_vec();
  let y_test = y[len_short..len - 1].to_vec();

  (x_train, x_test, y_train, y_test)
}

pub fn normalize_data(x: InputsVec) -> InputsVec {
  let mut mins: [f32; 9] = [11 as f32; 9];
  let mut maxs: [f32; 9] = [-1 as f32; 9];

  for a in x.iter() {
    for i in 0..9 {
      mins[i] = f32::min(mins[i], a[i]);
      maxs[i] = f32::min(maxs[i], a[i]);
    }
  }

  let mut xp: InputsVec = Vec::new();
  for a in x.iter() {
    let mut ap: [f32; 9] = [0 as f32; 9];
    for i in 0..9 {
      ap[i] = (a[i] - mins[i]) / (maxs[i] - mins[i]);
    }
    xp.push(ap);
  }
  xp
}

pub fn get_weights(graph: &Graph, model: &Model) -> HashMap<NodeIndex, Vec<f32>> {
  let weights_indices = params(&model);
  weights_indices
    .iter()
    .map(|index| {
      (
        index.clone(),
        graph
          .tensors
          .get(&(index.clone(), 0))
          .unwrap()
          .downcast_ref::<Vec<f32>>()
          .unwrap()
          .clone(),
      )
    })
    .collect()
}

pub struct TrainParams {
  pub data: (InputsVec, OutputsVec),
  pub epochs: usize,
  // pub lr: f32,
  // pub batch_size: u32,
  // pub model: Model,
}

/// Contains everything needed to define the snark: the ml graph but without the gradients, trained weights and indexes.
/// Note: this is quite a specific and frankly poor interface between training and snark synthesiz, so don't take it as engraved in stone.
#[derive(Debug)]
pub struct GraphForSnark {
  // the initial ml computation graph, without gradients
  pub graph: Graph,
  pub input_id: NodeIndex,
  pub weights: Vec<(NodeIndex, Vec<f32>)>,
}

impl GraphForSnark {
  pub fn copy_graph_roughly(&self) -> Self {
    let (g, remap) = copy_graph_roughly(&self.graph);
    GraphForSnark {
      graph: g,
      input_id: remap[&self.input_id],
      weights: self
        .weights
        .iter()
        .map(|(a, b)| (remap[a], b.clone()))
        .collect(),
    }
  }
}

/// Contains everything needed to define a snark and also evaluate the model.
/// Note: this is quite a specific and frankly poor interface between training and snark synthesiz, so don't take it as engraved in stone.
///       Generally: this is graph + some stuff recorded to evaluate it on input.
#[derive(Debug)]
pub struct TrainedGraph {
  /// the original ml computation graph, without gradients + input id + trained weights
  pub graph: GraphForSnark,
  // below are needed to evaluate the model to compare result against a snark derived from GraphForSnark:
  pub cx: Graph, /// full trained graph for evaluation, the above "graph" is similar but without gradients
  pub cx_weights: Vec<(NodeIndex, Vec<f32>)>, // needed for evaluation, mostly tests. redundant a bit
  pub cx_input_id: NodeIndex, // needed for evaluation, mostly tests
  pub cx_target_id: NodeIndex, // needed for evaluation, mostly tests
  pub cx_output_id: NodeIndex,
}

impl TrainedGraph {
  pub fn evaluate(&mut self, input_data: Vec<f32>) -> Vec<f32> {
    self.cx.get_op_mut::<Function>(self.cx_input_id).1 =
      Box::new(move |_| vec![Tensor::new(input_data.to_owned())]);
    self.cx.get_op_mut::<Function>(self.cx_target_id).1 =
      Box::new(move |_| vec![Tensor::new(vec![0.0])]); // doesnt matter
    let weights = self.cx_weights.clone();
    for (a, b) in weights {
      self.cx.get_op_mut::<Function>(a).1 = Box::new(move |_| vec![Tensor::new(b.clone())]);
    }
    self.cx.execute();
    let d = self
      .cx
      .get_tensor_ref(self.cx_output_id, 0)
      .unwrap()
      .clone()
      .downcast_ref::<Vec<f32>>()
      .unwrap()
      .clone();
    d
  }
}

pub fn run_model(train_params: TrainParams) -> TrainedGraph {
  let dataset: (InputsVec, OutputsVec) = train_params.data;
  let EPOCHS = train_params.epochs;
  // Setup gradient graph
  let mut cx = Graph::new();
  let model = <Model>::initialize(&mut cx);
  let input = cx.tensor::<R1<9>>();
  let output = model.forward(input).retrieve();

  // cx.display();
  // record graph without gradients. assuming nodeids dont change in Autograd::compile
  let (cx_og, remap) = copy_graph_roughly(&cx);
  let input_id = remap[&input.id];

  let target = cx.tensor::<R1<1>>();
  let loss = mse_loss(output, target).retrieve();
  let weights = params(&model);

  let grads = cx.compile(Autograd::new(&weights, loss), ());
  let (new_weights, lr) = sgd_on_graph(&mut cx, &weights, &grads);
  cx.keep_tensors(&new_weights);
  cx.keep_tensors(&weights);
  lr.set(5e-3);

  let (mut loss_avg, mut acc_avg) = (ExponentialAverage::new(1.0), ExponentialAverage::new(0.0));
  let start = std::time::Instant::now();
  // let EPOCHS = 20;

  let (X, Y) = dataset;
  let (X_train, _x_test, y_train, _y_test) = split_dataset(X, Y, 0.8);
  let X_train = normalize_data(X_train);
  let mut iter = 0;
  for _ in 0..EPOCHS {
    for (x, y) in zip(X_train.iter(), y_train.iter()) {
      let answer = [y.to_owned()];
      input.set(x.to_owned());
      target.set(answer);

      cx.execute();
      transfer_data_same_graph(&new_weights, &weights, &mut cx);
      loss_avg.update(loss.data()[0]);
      loss.drop();
      // println!("{:}, {:}", output.data()[0], answer[0]);
      acc_avg.update(
        output
          .data()
          .into_iter()
          .zip(answer)
          .filter(|(a, b)| (a - b).abs() < 0.5)
          .count() as f32,
      );
      output.drop();
      // println!(
      //   "Iter {iter} Loss: {:.2} Acc: {:.2}",
      //   loss_avg.value, acc_avg.value
      // );
      iter += 1;
    }
  }
  println!("Finished in {iter} iterations");
  println!(
    "Took {:.2}s, {:.2}µs / iter",
    start.elapsed().as_secs_f32(),
    start.elapsed().as_micros() / iter
  );
  // cx.display();
  let cx_weights_vec: Vec<(NodeIndex, Vec<f32>)> = weights
    .into_iter()
    .map(|a| {
      (
        a,
        cx.tensors
          .get(&(a, 0 /* assuming single output */))
          .unwrap()
          .downcast_ref::<Vec<f32>>()
          .unwrap()
          .clone()
          .into_iter()
          .collect(),
      )
    })
    .collect();
  let weights_vec = cx_weights_vec
    .iter()
    .map(|(a, b)| (remap[&a], b.clone()))
    .collect();
  // assert!(input_id == input.id);
  TrainedGraph {
    graph: GraphForSnark {
      graph: cx_og,
      weights: weights_vec,
      input_id,
    },
    cx: cx,
    cx_weights: cx_weights_vec,
    cx_output_id: output.id,
    cx_input_id: input.id,
    cx_target_id: target.id,
  }
}

pub struct ExponentialAverage {
  beta: f32,
  moment: f32,
  pub value: f32,
  t: i32,
}

impl ExponentialAverage {
  pub fn new(initial: f32) -> Self {
    ExponentialAverage {
      beta: 0.999,
      moment: 0.,
      value: initial,
      t: 0,
    }
  }
}

impl ExponentialAverage {
  pub fn update(&mut self, value: f32) {
    self.t += 1;
    self.moment = self.beta * self.moment + (1. - self.beta) * value;
    // bias correction
    self.value = self.moment / (1. - f32::powi(self.beta, self.t));
  }

  pub fn reset(&mut self) {
    self.moment = 0.;
    self.value = 0.0;
    self.t = 0;
  }
}
