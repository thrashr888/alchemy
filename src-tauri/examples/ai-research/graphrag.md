GraphRAG: From Local to Global - arXiv:2404.16130

Edge et al., 2024. Retrieval for the questions retrieval couldn't answer: the ones about the whole corpus.

Vector RAG answers local questions - the ones whose evidence sits in a handful of passages. Ask something global ("what are the main themes across these documents?") and nearest-neighbor search has nothing to grab: the answer is a property of the corpus, not of any chunk in it.

GraphRAG spends indexing-time compute to make global questions answerable. An LLM pass extracts entities and relationships from every chunk, building a knowledge graph; community detection then partitions the graph into clusters at several levels; and each community gets an LLM-written summary. A global query runs map-reduce over community summaries - each contributes partial answers, which are reduced into a final one. Comprehensiveness and diversity of answers beat vanilla RAG substantially in their evaluations.

The trade is honest and steep: indexing costs many LLM calls per corpus, and the pipeline's judge-scored wins invite skepticism. But the core observation - that some questions need pre-computed structure, not better retrieval - reframed how RAG systems are designed, this one included.

Read the paper: https://arxiv.org/abs/2404.16130
