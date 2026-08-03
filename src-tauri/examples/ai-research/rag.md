Retrieval-Augmented Generation for Knowledge-Intensive NLP - arXiv:2005.11401

Lewis et al., 2020. Named and formalized the pattern this application is built on.

Parametric models store what they know in their weights. That knowledge is hard to inspect, hard to update, and prone to confident fabrication when it is absent. RAG pairs a parametric generator with a non-parametric memory - a dense vector index over a document corpus - that it can query at generation time.

The architecture retrieves relevant passages for an input, then conditions generation on both the input and the retrieved text. The authors train the retriever and generator jointly, and compare two variants: one that conditions the whole output on a single retrieved document, and one that can draw on different documents for different tokens.

RAG set the state of the art on several open-domain question answering benchmarks, and produced more specific and more factual language than a comparable parametric-only baseline. The properties that made it endure are operational: the knowledge source can be swapped or updated without retraining, and every claim can be traced to a retrieved passage - which is precisely what makes grounded citation possible.

Read the paper: https://arxiv.org/abs/2005.11401
