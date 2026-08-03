Adam: A Method for Stochastic Optimization - arXiv:1412.6980

Kingma and Ba, 2014. The default optimizer for deep learning, still, a decade on.

Stochastic gradient descent needs a learning rate, and a single global one suits some parameters badly. Two earlier ideas addressed this separately: momentum accumulates a running average of gradients to smooth the descent direction, and AdaGrad/RMSProp scale each parameter's step by a running estimate of its gradient magnitude, so rarely-updated parameters take larger steps.

Adam combines them. It maintains exponential moving averages of both the gradient (first moment) and its square (second moment), corrects both for the bias introduced by initializing them at zero, and uses their ratio as a per-parameter step. The result is an optimizer that needs relatively little tuning, handles sparse gradients and non-stationary objectives, and is invariant to diagonal rescaling of the gradients.

The paper's practical appeal was that the defaults usually work. That is a large part of why Adam and its variants - AdamW especially, which decouples weight decay from the gradient update - remain the first thing practitioners reach for.

Read the paper: https://arxiv.org/abs/1412.6980
