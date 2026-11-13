from . import triviumdb as _native
from ._arguments import install as _install_arguments

_install_arguments(_native)

from .triviumdb import *
from .triviumdb import __all__

del _install_arguments, _native
