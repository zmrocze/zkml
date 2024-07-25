# zkml

## ZK

Zero-knowledge proofs (zk-snarks) are cryptographic tricks to - in here - prove to the public that I have a private value, such that a public computation on the value returns such and such value.

That is we agree on some computation - like producing a hash - and then what someone can do is to make a prove that he knows a value that hashes to a certain hash, BUT he doesn't need to share that value (otherwise it would make no sense). To do that the computation needs to be encoded in a certain way and prepared beforehand, only later can proves be created and verified.

The proofs are zero-knowledge, because eventhough a recipient of a proof can verify its claim - he gets no hint at all about that private argument to the computation that was used by the prover.

## ML

Machine learning models are in essence fixed algebraic circuits, operating on arrays of floats with various numerical operations - like addition or trigonometric functions. The usefulness comes from the fact that these can be trained for an objective using gradient descent. 

We are concerned with an already trained neural network, used for inference.

## ZKML

Here we rewrite a neural network to a form that makes it possible to produce zk-snark's from it. That is the neural net will produce inferences on private data, together with proofs of the results.

So imagine there is a neural network that classifies images: looks at an image of a shopping bag and a receipt and verifies if the receipt matches the shopping bag. The neural network is public, the grocery store has shared it with clients and sets the rule: you can leave our store if the neural network allows. What we can now do with this library is to make ourselves a photo and then create a proof certifying that the neural network accepts our photo. We can share the proof with the grocery store, but keep our photo (and with it our shopping list) private.

The above example has an obvious problem (when did i make the photo?), but even more importantly - this solution is not at all enough efficient computationaly to make such usecase possible. Therefore this project should be treated more like a proof of concept - but with computational advancements and generalization to multi-party computations, this presents exciting possibilities.

Here, either the weights or the inputs or any combination of the two can be made private/public.

### Demo

To see how it's used and how it works, check out the tests in [lib::test_trained_into_snark](https://github.com/przyjacielpkp/zkml/blob/main/lib/src/lib.rs#L76). Or run them with `cargo test --profile=test`. Tests take some minute and a half to run on my laptop.

These demonstrate the full functionality: 
 - a trained model is taken, that is a computation graph with the weight assignments (and some more bookeeping, we admit the abstraction is leaky here)
 - the snark public key gets calculated
 - prover provides the private argument and calculates the proof, together with evaluation of the circuit that is being claimed
 - the evaluation when mapped back to float matches the evaluation of the original non-snark computation

### How does it work

Machine learning models are fixed algebraic circuits, operating on arrays of floats.
The native model of computations which are possible to be made into a snark way is a polynomial computation over a prime field.

This means two things:

 - Need to encode floats as prime field elements. This is done by scaling, for details see [[Note: floats as ints]](https://github.com/przyjacielpkp/zkml/blob/c678d410adc3de188ce439b94ad4b9edba7785cf/lib/src/snark/snark.rs#L124).
 - Need to rewrite vectorized computations to significantly more scalar computions. This is done and described in [the scalar module](https://github.com/przyjacielpkp/zkml/blob/c678d410adc3de188ce439b94ad4b9edba7785cf/lib/src/scalar.rs#L131).

For a description of what we're dealing with initially as the input ml computation, see [[Note: graph representation]](https://github.com/przyjacielpkp/zkml/blob/c678d410adc3de188ce439b94ad4b9edba7785cf/lib/src/scalar.rs#L31). This is a general and simple representation, similar abstraction level to tinygrad, onnx, pytorch or tensorflow models map onto it (provided these're defined statically, see [pytorch docs](https://pytorch.org/tutorials/intermediate/torch_export_tutorial.html#graph-breaks)).
