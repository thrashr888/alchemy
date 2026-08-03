Generative Adversarial Networks - arXiv:1406.2661

Goodfellow et al., 2014. Framed generative modeling as a game between two networks.

Generative models before this generally required either an explicit tractable likelihood or expensive approximate inference. The adversarial framework avoids both. A generator maps random noise to samples; a discriminator tries to tell real data from generated data. The two train simultaneously, the generator improving by fooling the discriminator, the discriminator improving by catching it.

The authors show that this two-player minimax game has a global optimum where the generator's distribution matches the data distribution and the discriminator is maximally uncertain everywhere. Training needs no Markov chains and no inference network - just backpropagation through both models.

GANs produced the first widely convincing synthetic photographs and dominated image generation for several years, though they were notoriously unstable to train and prone to mode collapse. Diffusion models have since largely displaced them for high-fidelity generation, but the adversarial idea remains influential well beyond images.

Read the paper: https://arxiv.org/abs/1406.2661
