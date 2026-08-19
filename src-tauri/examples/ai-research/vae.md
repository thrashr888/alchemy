Auto-Encoding Variational Bayes (VAE) - arXiv:1312.6114

Kingma & Welling, 2013. Made latent-variable generative models trainable by gradient descent, and gave deep learning its first principled generator.

The problem is old: fit a model with continuous latent variables to data when the posterior over those latents is intractable. Classical variational methods existed, but each new model meant new derivations, and sampling-based gradients were too noisy to train neural networks.

The paper's move is the reparameterization trick. Instead of sampling a latent directly - which blocks gradients - sample noise from a fixed distribution and compute the latent as a deterministic function of that noise and the encoder's output. Gradients now flow through the sampling step, so an encoder network and a decoder network can be trained jointly to maximize a variational lower bound on the data likelihood, with ordinary backpropagation.

The result is the variational autoencoder: an encoder that maps data to a distribution over a latent space, and a decoder that maps latent points back to data. Its blurry samples were quickly outdone by GANs, but the framework endured - the latent spaces of today's diffusion models, and the "variational" in half of modern generative modeling, trace to this bound.

Read the paper: https://arxiv.org/abs/1312.6114
