///
/// Rewrite the ml source graph to scalarised version, where tensor operations are replaced with a multiplicity of scalar operations.
///  

/// Note: tensors may have dynamic dimensions.
/// Solution 1: assume not
// Problem: What about nodes that output multiple values? Add, Mul, LessThan, ReduceAdd - are not like that right?
use luminal::graph::Graph;

use std::{collections::HashMap, error::Error, fs::File, io::Write};

use itertools::Itertools;
use petgraph::{
  graph::EdgeIndex,
  visit::{EdgeRef, IntoNodeIdentifiers},
  Direction::{Incoming, Outgoing},
};
use tracing::{debug, info, instrument, warn};

use luminal::{
  op::{Constant, InputTensor, Operator},
  prelude::*,
  shape::Shape,
};

use crate::model::copy_graph_roughly;

/// Asserts (in non-strictly-typed way) that all input tensors are single values.
#[derive(Debug)]
pub struct ScalarGraph {
  /// Note Graph representation:
  ///   Graph is a DAG of the expression defining a tensor computation.
  ///   
  ///   Nodes keep weights signifying operations. You check the weight by type assertions on the node weight.
  ///   Edges keep the shape incoming from node to node. That means that an incoming edge:
  ///     - records the index in argument list to the operation in the target node
  ///     - records an expression that maps logical tensor indices in the incoming tensor to physical indices in what will be evaluated in source node
  ///     - records the shape (n,) of the physical tensor in what will be evaluated in source node
  ///
  ///   As we are concerned with a snark computation derived from the graph here,
  ///   we don't care about evaluation step. We are only concerned about rewrites of the static computation graph.
  ///
  ///   Scalar: means all shapes at edges are (1,).
  pub graph: Graph,
  /// In the rewrite to scalar we substitute nodes for multiple nodes, here's a mapping tracking that.
  pub inputs_tracker: InputsTracker,
}

impl ScalarGraph {
  pub fn copy_graph_roughly(&self) -> Self {
    ScalarGraph {
      graph: copy_graph_roughly(&self.graph),
      inputs_tracker: self.inputs_tracker.clone(),
    }
  }
}

/// Rewrite the static tensor computation to scalar computation.
pub fn scalar(mut cx: Graph) -> ScalarGraph {
  // TODO: unfortunetely original cx is destroyed in the process
  // let mut cx1 = (&cx).clone().clone();
  // we dont care about remap for now
  let mut remap: Vec<NodeIndex> = vec![];
  let ((), inputs_tracker) = cx.compile(ScalarCompiler::default(), &mut remap);
  ScalarGraph {
    graph: cx,
    inputs_tracker,
  }
}

#[derive(Debug, Default)]
pub struct UniformOutShapes;

/// Kinda obsolete
/// This step doesn't modify the graph, only asserts a property (uniform shapes).
/// We keep it out of interest in whether it succeeds. Comment out the moment it fails and we know why.
impl Compiler for UniformOutShapes {
  type Output = ();
  #[instrument(level = "debug", skip(ids))]
  fn compile<T: ToIdsMut>(&self, graph: &mut Graph, ids: T) -> Self::Output {
    // For every node substitute as many copies of it as there are distinct outgoing shapes.
    // Connect the new nodes to the target nodes correspondingly wrt shapes.

    // Assuming : output_in = 0
    // Shapes could actually be different

    debug!("Assuming from every node all outgoing edges are of same shape and output_in.");
    for node in graph.graph.node_indices() {
      let all_equal = graph
        .graph
        .edges_directed(node, Outgoing)
        .filter_map(|e| e.weight().as_data().map(|w| (w.1 /* hope equal 0 */, w.2)))
        .all_equal();
      assert!(
        all_equal,
        "All outgoing edges of a node must have the same shape."
      )
    }
  }
}

pub type ScalarCompiler = (UniformOutShapes, Scalarize);

#[derive(Debug, Default, Clone)]
/// In the scalar graph used for source nodes no matter they original Op.
pub struct InputOp {}

impl Operator for InputOp {
  fn process(&mut self, _inp: Vec<(InputTensor, ShapeTracker)>) -> Vec<Tensor> {
    panic!("InputOp: We wont be evaluating it either way")
  }
}

#[derive(Debug, Default, Clone)]
pub struct ConstantOp {}

impl Operator for ConstantOp {
  fn process(&mut self, _inp: Vec<(InputTensor, ShapeTracker)>) -> Vec<Tensor> {
    panic!("InputOp: We wont be evaluating it either way")
  }
}

// TODO: rewrite to lessthan? treat in snark? ignore?
#[derive(Debug, Default, Clone)]
pub struct Max {}

impl Operator for Max {
  fn process(&mut self, _inp: Vec<(InputTensor, ShapeTracker)>) -> Vec<Tensor> {
    panic!("Max op: We wont be evaluating it either way")
  }
}

#[derive(Debug, Default, Clone)]
/// Remembers how to supply inputs to scalar graph to match inputs to tensor graph.
/// Tracks inputs and constant.
pub struct InputsTracker {
  /// If x was of shape (2, 3) then new_inputs[x] should be a vector of length 6
  pub new_inputs: HashMap<NodeIndex, Vec<NodeIndex>>,
  pub constants: HashMap<NodeIndex, f32>,
}

#[derive(Debug, Default)]
pub struct Scalarize;

impl Compiler for Scalarize {
  type Output = InputsTracker;

  // THIS-WORKS

  #[instrument(level = "debug", name = "compile", skip(_ids))]
  /// Start from the sinks in graph and go backwards.
  /// Look at a node - assume all its outgoing shapes are the same (due to UniformOutShapes).
  /// We want to rewrite it to many little nodes.
  /// From previous steps the outgoing edges are already multiplied into shape many edges.
  /// We want to create shape many little nodes with outputs (and as many as needed nodes to implement the rest of the circuit).
  /// We connect the outgoing edges to corresponding little nodes using indices like with tensors.
  /// We create edges connecting our little nodes to source nodes. For every source there will source's shape many edges going from that source.
  fn compile<T: ToIdsMut>(&self, graph: &mut Graph, mut _ids: T) -> InputsTracker {
    // Assumes that all outgoing edges have same shape from a given node. NOTE: why? not needed once realized physical shape is always going to be same for single output.
    // FIX: ^ Not true.

    // Q: do inefficient but simpler with Looped<(AddCompile, MulCompile)> etc and pattern matching
    //    or efficiently in a single for loop in toposort order (and meticoulous manual pattern matching)
    // A: option 2, because cant do 1 efficiently

    // TODO: What about ops returning many tensors? (no prim ops right?)
    // Problem: We decide little nodes amount based on outgoing shape, assuming there's one tensor produced.

    // mark retrieve nodes
    let mark_retrieve = |x: &NodeIndex, new_xs: Vec<_>, g: &mut Graph| {
      if let Some(w) = g.to_retrieve.get(x) {
        assert!(w.0 == 0, "Assuming single output");
        for new_x in new_xs {
          // let new_x : NodeIndex = new_x;
          g.to_retrieve.insert(
            new_x,
            (
              0, /* this probably refers to output index in Vec<Tensor> */
              R0::to_tracker(),
            ),
          );
        }
      }
    };

    let get_own_size = |x, gg: &Graph| {
      let get_own_shape = |x, gg: &Graph| {
        // reasonably we expect one of two cases: there is some outgoing edge OR it is a retrieval node
        if let Some(w) = gg.to_retrieve.get(&x) {
          w.clone().1
        } else {
          match gg
            .edges_directed(x.clone(), Outgoing)
            .filter_map(|e| e.weight().as_data())
            .next()
          {
            Some((_, _, shape)) => shape,
            None => {
              panic!("Add node has no outgoing edges and is not a retrieval node.")
            }
          }
        }
      };
      // assuming (and we have to) a staticly known shape
      match get_own_shape(x, gg).n_physical_elements().to_usize() {
        Some(n) => n,
        None => {
          panic!("Node's output shape is not static.")
        }
      }
    };

    // We split node into multiple nodes instead.
    // This helper creates the little nodes all with same Op.
    fn make_nodes<T: Operator + 'static + Clone>(
      size: usize,
      op: T,
      graph: &mut Graph,
    ) -> Vec<NodeIndex> {
      let mut little_nodes = vec![];
      for _ in 0..size {
        little_nodes.push(graph.add_op(op.clone()).finish())
      }
      little_nodes
    }

    /// When looking at node x, already the outgoing edges are created and wired to little circuit created when substituting for nodes previous to x.
    /// This helper connects these edges to <x physical shape> many little nodes.
    fn connect_out_edges(
      x: NodeIndex,
      little_nodes: &Vec<NodeIndex>,
      edge_src_indices: &HashMap<EdgeIndex, usize>,
      graph: &mut Graph,
    ) {
      let out_edges: Vec<_> = graph
        .graph
        .edges_directed(x, Outgoing)
        .filter_map(|e| e.weight().as_data().map(|d| (e.id(), d, e.target())))
        .collect();

      for (e, (input_order, output_order, shape), target) in out_edges {
        let logical_index = edge_src_indices[&e];
        // using output_order as the remembered index in logical shape
        // TODO: not recalculate the index_expressions so much
        let phys_index = match logical_to_physical(
          &(shape.index_expression(), shape.valid_expression()),
          logical_index,
        ) {
          Some(i) => i,
          None => {
            panic!("Something fucked up, outgoing edge index outside of expected physical size")
          }
        };
        info!(
          "phys={:?}, i={:?}, output={:?}",
          phys_index, input_order, output_order
        );
        graph.add_edge(
          little_nodes[phys_index],
          target,
          Dependency::Data {
            input_order,
            output_order,
            shape: R0::to_tracker(),
          },
        );
      }
    }

    fn pointwise_op<T: Operator + 'static + Clone>(
      op: T,
      x: NodeIndex,
      size: usize,
      incoming: &Vec<(EdgeIndex, (u8, u8, ShapeTracker), NodeIndex)>,
      edge_src_indices: &mut HashMap<EdgeIndex, usize>,
      graph: &mut Graph,
    ) -> Vec<NodeIndex> {
      let little_nodes = make_nodes(size, op, graph);
      connect_out_edges(x, &little_nodes, edge_src_indices, graph);

      for (e, (b, output_order, shape), source) in incoming {
        // assert!(*output_order == 0, "Assuming sigle valued Op's"); // actually idk if we do
        // assuming static shape
        let k = shape.n_elements().to_usize().unwrap();
        assert!( k == size, "Expected physical shape to be the same as incoming logical shape. size = {}, k = {}, src = {:?}", size, k, source ); // Op specific
        for j in 0..k {
          let (from, to) = (j, j); // pointwise
          debug!("k={:?}, j={:?}, b={:?}", k, j, b);
          edge_src_indices.insert(*e, from);
          graph.add_edge(
            source.clone(),
            little_nodes[to],
            Dependency::Data {
              input_order: *b as u8,
              output_order: *output_order,
              shape: *shape, // saving the original shape
            },
          );
        }
      }
      little_nodes
    }

    fn reduce_op<T: Operator + 'static + Clone>(
      op: T,
      x: NodeIndex,
      size: usize,
      ax: usize, /* reduce axis */
      yy: &(EdgeIndex, (u8, u8, ShapeTracker), NodeIndex),
      edge_src_indices: &mut HashMap<EdgeIndex, usize>,
      graph: &mut Graph,
    ) -> Vec<NodeIndex> {
      let (_, (_, from_output, sh), y) = yy;
      let dims = sh.shape_usize();
      let ax_len = dims[ax];
      let front_size = dims.iter().take(ax).product::<usize>().max(1);
      let back_size = dims.iter().skip(ax + 1).product::<usize>().max(1);
      assert!(
        ax_len > 1,
        "Why reducing scalar? but also im lazy to implement that edgecase."
      );
      assert!(*from_output == 0, "Thats not strictly necessary but 1) is always the case 2) is needed for this lazy implementation." );
      assert!(
        size == sh.n_elements().to_usize().unwrap() / ax_len,
        "Expect result size to be the size after collapsing the ax dim."
      );
      assert!(size == front_size * back_size);
      let create_reduce_circuit = |i| {
        let front_i = i / front_size;
        let back_i = i % front_size;
        assert!(front_i * front_size + back_i == i);
        let xs = (0..ax_len).map(|k| {
          front_i * back_size * ax_len + k * back_size + back_i // index in y of k-th element in current axe
        });
        xs.fold(*y, |l_node, k| {
          let new = graph.add_op(op.clone()).finish();
          let _ = graph.add_edge(
            l_node,
            new,
            Dependency::Data {
              input_order: 0,
              output_order: 0, /* assuming yy outputs one vector */
              shape: R0::to_tracker(),
            },
          );
          let e_r = graph.add_edge(
            *y,
            new,
            Dependency::Data {
              input_order: 1,
              output_order: 0, /* assuming yy outputs one vector */
              shape: R0::to_tracker(),
            },
          );
          edge_src_indices.insert(e_r, k); /* recording logical index of a scalar edge */
          new
        })
      };
      let little_nodes: Vec<NodeIndex> = (0..size).map(create_reduce_circuit).collect();
      connect_out_edges(x, &little_nodes, &edge_src_indices, graph);
      little_nodes
    }

    let mut inputs_tracker = InputsTracker::default();

    // precalculate all physical sizes as we're going to be removing edges
    let sizes = graph
      .node_identifiers()
      .map(|x| (x, get_own_size(x, graph)))
      .collect::<HashMap<_, _>>();

    // when creating an edge targeting a newly made little node we need to remember for what index in the incoming shape it was made
    let mut edge_src_indices: HashMap<EdgeIndex, usize> = HashMap::new();

    let pi = {
      let mut pi = petgraph::algo::toposort(&graph.graph, None).unwrap();
      pi.reverse();
      pi
    };

    // for every node:
    // 0. Match x on Op and arity
    // 1. Create pack of little nodes replacing x
    // 2. Connect outgoing edges, based on indices of the edges which from previous step are indexed like shape's logical indexes
    // 3. Create edges for incoming edges, connect as needed by the Op. Record wanted src index in map.
    // 4. Remove x. Mark the new nodes for retrieval.
    for x in pi {
      // Invariant of the loop:
      //  - all nodes upstream from x (later in toposort) were already substituted for many scalar nodes.
      //  - the outgoing edges are of scalar shape and we have recorded *what physical index in the result of x the edge connects to*
      info!("x={:?} in g={:?}", x, graph.graph);

      let incoming: Vec<_> = graph
        .edges_directed(x, Incoming)
        .filter_map(|e| e.weight().as_data().map(|d| (e.id(), d, e.source())))
        .sorted_by_key(|(_, (inp, _, _), _)| *inp)
        .collect();
      let size = sizes[&x];

      let little_nodes = if incoming.is_empty() {
        // x is source
        if graph.check_node_type::<Function>(x) {
          // Function op could be in anything but as a source node in practical terms it means an input.
          let little_nodes = make_nodes(size, InputOp {}, graph);
          connect_out_edges(x, &little_nodes, &edge_src_indices, graph);
          inputs_tracker.new_inputs.insert(x, little_nodes.clone());
          little_nodes
        } else if graph.check_node_type::<Constant>(x) {
          let little_nodes = make_nodes(size, ConstantOp {}, graph);
          connect_out_edges(x, &little_nodes, &edge_src_indices, graph);
          let val = graph.node_weight_mut(x).unwrap().process(vec![])[0]
            .downcast_ref::<Vec<f32>>()
            .unwrap()
            .clone()[0];
          assert!(
            little_nodes.len() == 1,
            "Constants are expected to be scalars"
          );
          little_nodes.iter().for_each(|y| {
            inputs_tracker.constants.insert(*y, val);
          });
          little_nodes
        } else {
          panic!("Unsupported source node type!")
        }
      } else if let Some((yy,)) = incoming.iter().collect_tuple() {
        if graph.check_node_type::<Recip>(x) {
          // same as Add but unop
          // TODO: this works right???
          pointwise_op(Recip {}, x, size, &incoming, &mut edge_src_indices, graph)
        } else if graph.check_node_type::<SumReduce>(x) {
          let ax: &SumReduce = graph
            .node_weight(x)
            .unwrap()
            .as_any()
            .downcast_ref()
            .unwrap();
          reduce_op(Add {}, x, size, ax.0, yy, &mut edge_src_indices, graph)
        } else if graph.check_node_type::<MaxReduce>(x) {
          let ax: &MaxReduce = graph
            .node_weight(x)
            .unwrap()
            .as_any()
            .downcast_ref()
            .unwrap();
          reduce_op(Max {}, x, size, ax.0, yy, &mut edge_src_indices, graph)
        } else {
          panic!("Unsupported unop OP")
        }
      }
      // x is binop
      else if let Some((ll, rr)) = incoming.iter().collect_tuple() {
        if graph.check_node_type::<Add>(x) {
          debug!("Add {:?} {:?}", ll, rr);
          pointwise_op(Add {}, x, size, &incoming, &mut edge_src_indices, graph)
        } else if graph.check_node_type::<Mul>(x) {
          debug!("Mul {:?} {:?}", ll, rr);
          pointwise_op(Mul {}, x, size, &incoming, &mut edge_src_indices, graph)
        } else if graph.check_node_type::<LessThan>(x) {
          debug!("LessThan {:?} {:?}", ll, rr);
          pointwise_op(
            LessThan {},
            x,
            size,
            &incoming,
            &mut edge_src_indices,
            graph,
          )
        } else {
          todo!("Unsupported yet binop!") // are there any other binops we need?
        }
      } else {
        // TODO: error handling
        panic!("unexpected node type")
      };

      // !!!
      mark_retrieve(&x, little_nodes, graph);
      graph.remove_node(x);
    }

    return inputs_tracker;
  }
}

pub fn save_graphviz(path: String, graph: &Graph) -> Result<(), Box<dyn Error>> {
  use petgraph::dot::Dot;
  let dot = Dot::with_config(&graph.graph, &[]);
  let mut file = File::create(path)?;
  write!(file, "{:?}", dot)?;
  Ok(())
}

pub fn pretty_print_g(graph: &Graph) -> Result<(), Box<dyn Error>> {
  // TODO

  use petgraph_graphml::GraphMl;
  let a = GraphMl::new(&graph.graph).pretty_print(true);
  let mut str: Vec<u8> = vec![];
  let x = a.to_writer(&mut str)?;
  let str = String::from_utf8(str)?;
  // let str1 = str.as_ascii().into_iter().map(|x| x.clone()).collect::<Vec<_>>();
  println!("pretty g = {:?}", str);

  Ok(())
}

#[cfg(test)]
mod tests {
  use std::error::Error;

  use luminal::{
    graph::Graph,
    shape::{Const, R1, R2},
  };
  use tracing::info;

  use crate::{
    scalar::{pretty_print_g, save_graphviz},
    utils,
  };

  use super::ScalarCompiler;

  #[test]
  fn test_run() -> Result<(), Box<dyn Error>> {
    utils::init_logging()?;
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<2>>().set(vec![1.0, 1.0]);
    let b = cx.tensor::<R1<2>>().set(vec![2.0, 2.0]);
    let d = cx.tensor::<R1<2>>().set(vec![3.0, 3.0]);
    let mut c = ((a + b) + d).retrieve();
    print!("{:?}", cx);
    save_graphviz("test_run_tensor.dot".to_string(), &cx)?;
    let r = cx.compile(ScalarCompiler::default(), &mut c);
    print!("{:?}", cx);
    print!("{:?}", r);
    // pretty_print_g(&cx)?;
    save_graphviz("test_run_scalar.dot".to_string(), &cx)?;
    cx.display();
    info!("compiled : {:?}", cx.graph);

    // THIS-WORKS
    // Open to see original graph  of (a+b)+d:
    // https://dreampuf.github.io/GraphvizOnline/#digraph%20%7B%0A%20%20%20%200%20%5B%20label%20%3D%20%22Tensor%20Load%22%20%5D%0A%20%20%20%201%20%5B%20label%20%3D%20%22Tensor%20Load%22%20%5D%0A%20%20%20%202%20%5B%20label%20%3D%20%22Tensor%20Load%22%20%5D%0A%20%20%20%203%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%204%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%200%20-%3E%203%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B2%5D%2C%20indexes%3A%20%5B0%5D%2C%20fake%3A%20%5Bf%0Aalse%5D%2C%20mask%3A%20%5B(0%2C%202147483647)%5D%2C%20padding%3A%20%5B(0%2C%200)%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%201%20-%3E%203%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B2%5D%2C%20indexes%3A%20%5B0%5D%2C%20fake%3A%20%5Bf%0Aalse%5D%2C%20mask%3A%20%5B(0%2C%202147483647)%5D%2C%20padding%3A%20%5B(0%2C%200)%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%203%20-%3E%204%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B2%5D%2C%20indexes%3A%20%5B0%5D%2C%20fake%3A%20%5Bf%0Aalse%5D%2C%20mask%3A%20%5B(0%2C%202147483647)%5D%2C%20padding%3A%20%5B(0%2C%200)%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%202%20-%3E%204%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B2%5D%2C%20indexes%3A%20%5B0%5D%2C%20fake%3A%20%5Bf%0Aalse%5D%2C%20mask%3A%20%5B(0%2C%202147483647)%5D%2C%20padding%3A%20%5B(0%2C%200)%5D%20%7D%20%7D%22%20%5D%0A%7D%0A
    // Open for scalar graph (see its basically double the original in this case):
    // https://dreampuf.github.io/GraphvizOnline/#digraph%20%7B%0A%20%20%20%200%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%201%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%203%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%204%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%205%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%206%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%207%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%208%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%209%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2010%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%208%20-%3E%207%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%203%20-%3E%204%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%207%20-%3E%206%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%204%20-%3E%205%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%209%20-%3E%207%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%200%20-%3E%204%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%2010%20-%3E%206%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%0A%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%201%20-%3E%205%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%7D%0A%0A

    Ok(())
  }

  #[test]
  fn test_run_2() -> Result<(), Box<dyn Error>> {
    utils::init_logging()?;
    let mut cx = Graph::new();
    let a = cx.tensor::<R1<2>>().set(vec![4.0, 4.0]);
    let b = cx.tensor::<R1<2>>().set(vec![8.0, 8.0]);
    let d = cx
      .tensor::<R2<2, 3>>()
      .set(vec![1.0, 2.0, 3.0, 1.0, 2.0, 3.0]);
    let mut c = ((a + b).expand::<(_, Const<3>), _>() + d).retrieve();
    print!("{:?}", cx);
    save_graphviz("test_run2_tensor.dot".to_string(), &cx)?;
    let r = cx.compile(ScalarCompiler::default(), &mut c);
    print!("{:?}", cx);
    print!("{:?}", r);
    // pretty_print_g(&cx)?;
    save_graphviz("test_run2_scalar.dot".to_string(), &cx)?;
    cx.display();
    info!("compiled : {:?}", cx.graph);

    // THIS-WORKS
    // Open to see original graph of (a+b).expand()+d:
    // https://dreampuf.github.io/GraphvizOnline/#digraph%20%7B%0A%20%20%20%200%20%5B%20label%20%3D%20%22Tensor%20Load%22%20%5D%0A%20%20%20%201%20%5B%20label%20%3D%20%22Tensor%20Load%22%20%5D%0A%20%20%20%202%20%5B%20label%20%3D%20%22Tensor%20Load%22%20%5D%0A%20%20%20%203%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%204%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%200%20-%3E%203%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B2%5D%2C%20indexes%3A%20%5B0%5D%2C%20fake%3A%20%5Bf%0Aalse%5D%2C%20mask%3A%20%5B(0%2C%202147483647)%5D%2C%20padding%3A%20%5B(0%2C%200)%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%201%20-%3E%203%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B2%5D%2C%20indexes%3A%20%5B0%5D%2C%20fake%3A%20%5Bf%0Aalse%5D%2C%20mask%3A%20%5B(0%2C%202147483647)%5D%2C%20padding%3A%20%5B(0%2C%200)%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%203%20-%3E%204%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B2%2C%203%5D%2C%20indexes%3A%20%5B0%2C%201%5D%2C%20fa%0Ake%3A%20%5Bfalse%2C%20true%5D%2C%20mask%3A%20%5B(0%2C%202147483647)%2C%20(0%2C%202147483647)%5D%2C%20padding%3A%20%5B(0%2C%200)%2C%20(0%2C%200)%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%202%20-%3E%204%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B2%2C%203%5D%2C%20indexes%3A%20%5B0%2C%201%5D%2C%20fa%0Ake%3A%20%5Bfalse%2C%20false%5D%2C%20mask%3A%20%5B(0%2C%202147483647)%2C%20(0%2C%202147483647)%5D%2C%20padding%3A%20%5B(0%2C%200)%2C%20(0%2C%200)%5D%20%7D%20%7D%22%20%5D%0A%7D%0A
    // Notice how it's the same graph as in test_run, but different shape at an edge
    // Open for scalar graph:
    // https://dreampuf.github.io/GraphvizOnline/#digraph%20%7B%0A%20%20%20%200%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%201%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%203%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%204%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%205%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%206%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%207%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%208%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%209%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2010%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2011%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2012%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2013%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2014%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2015%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2016%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2017%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2018%20%5B%20label%20%3D%20%22Add%22%20%5D%0A%20%20%20%2012%20-%3E%2011%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%0A%2C%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%203%20-%3E%204%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%2011%20-%3E%2010%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%0A%2C%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%2011%20-%3E%209%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%0A%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%2016%20-%3E%208%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%0A%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%2015%20-%3E%207%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%0A%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%2014%20-%3E%206%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%0A%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%201%20-%3E%205%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%2011%20-%3E%208%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%0A%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%204%20-%3E%207%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%204%20-%3E%206%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%204%20-%3E%205%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%200%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%2013%20-%3E%2011%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%0A%2C%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%200%20-%3E%204%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%20%0Amask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%2018%20-%3E%2010%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%0A%2C%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%20%20%20%2017%20-%3E%209%20%5B%20label%20%3D%20%22Data%20%7B%20input_order%3A%201%2C%20output_order%3A%200%2C%20shape%3A%20ShapeTracker%20%7B%20dims%3A%20%5B%5D%2C%20indexes%3A%20%5B%5D%2C%20fake%3A%20%5B%5D%2C%0A%20mask%3A%20%5B%5D%2C%20padding%3A%20%5B%5D%20%7D%20%7D%22%20%5D%0A%7D%0A

    Ok(())
  }
}

#[cfg(test)]
mod tests_other {
  use rand::{rngs::StdRng, SeedableRng};

  use luminal::prelude::*;

  use crate::scalar::ScalarCompiler;
  luminal::test_imports!();

  #[test]
  fn test_matmul() {
    let mut cx = Graph::new();
    let a = cx.tensor::<(Dyn<'M'>, Dyn<'K'>)>();
    let b = cx.tensor::<(Dyn<'K'>, Dyn<'N'>)>();
    let mut c = a.matmul(b).retrieve();

    cx.compile(ScalarCompiler::default(), &mut c);

    let d_dev = dfdx::prelude::Cpu::default();
    for m in (1..23).step_by(4) {
      for k in (1..35).step_by(3) {
        for n in (1..70).step_by(7) {
          let mut rng = StdRng::seed_from_u64(0);
          let a_data = random_vec_rng(m * k, &mut rng);
          let b_data = random_vec_rng(k * n, &mut rng);
          a.set_dyn(a_data.clone(), &[m, k]);
          b.set_dyn(b_data.clone(), &[k, n]);

          cx.execute();

          let d_a = d_dev.tensor_from_vec(a_data, (m, k));
          let d_b = d_dev.tensor_from_vec(b_data, (k, n));
          let d_c = d_a.matmul(d_b);

          assert_close_precision(&c.data(), &d_c.to_dtype::<f32>().as_vec(), 1e-2);
          c.drop();
        }
      }
    }
  }

  #[test]
  fn test_cpu_matmul_2d_2() {
    let mut cx = Graph::new();
    let a = cx.tensor::<R2<2, 3>>();
    a.set(vec![1.0, 2.0, 3.0, 1.0, 2.0, 3.0]);
    let b = cx.tensor::<R2<3, 4>>();
    b.set(vec![1., 2., 3., 1., 2., 3., 1., 2., 3., 1., 2., 3.]);
    let mut c = a.matmul(b).retrieve();

    cx.execute();

    let unoptimized_c = c.data();
    cx.compile(ScalarCompiler::default(), &mut c);
    cx.execute();
    assert_close(&c.data(), &unoptimized_c);
  }
}

fn logical_to_physical((ind, val): &(BigExpression, BigExpression), index: usize) -> Option<usize> {
  if val.exec_single_var(index) != 0 {
    Some(ind.exec_single_var(index))
  } else {
    None
  }
}
