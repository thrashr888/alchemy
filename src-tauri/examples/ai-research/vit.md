An Image is Worth 16x16 Words (Vision Transformer) - arXiv:2010.11929

Dosovitskiy et al., 2020. Showed that convolutions are not required for state-of-the-art image classification.

Convolutional inductive biases - locality, translation equivariance, hierarchical feature maps - were assumed essential for vision. The Vision Transformer discards them almost entirely. It cuts an image into fixed-size patches, flattens each into a vector, embeds them, adds positional embeddings, and feeds the resulting sequence to a standard Transformer encoder. The image is treated, quite literally, as a sentence of patches.

The important finding is about data. Trained on mid-sized datasets like ImageNet alone, ViT underperforms comparable ResNets - without convolutional priors it has to learn locality from examples. Pre-trained on much larger datasets and then transferred, it matches or beats the best convolutional networks at substantially lower training cost.

That trade - inductive bias versus data - is the paper's real contribution, and it generalized. It also unified the architecture story across modalities, which is much of what made large multimodal models straightforward to build.

Read the paper: https://arxiv.org/abs/2010.11929
