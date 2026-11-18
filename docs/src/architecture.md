# Architecture

This project is developed in a style of literate programming using [Entangled](https://entangled.github.io). All sources to the working program are present in this documentation.

Brei started out as a Python script supporting the use of Entangled by providing a simple and cheap workflow engine. The Python version uses Asyncio futures to perform the lazy evaluation of the tasks in a workflow. When porting to Rust, I found that it was easier to write a kernel that walks the dependency tree explicitly. This kernel design is still at the core of Brei (see [the engine implementation](./engine.md)), called Noodles after an earlier brainchild of mine by the same name.

This design was finalized and made working by Claude Opus 5.5, who also implemented all of the layering on top of the engine.

[Main Module](./main_module.md)
  : The CLI, logging and end-to-end testing
  
[Program Loader](./program_loader.md)
  : Data structures for program input
  
[Engine](./engine.md)
  : Core task machine
  
[Brei Rules](./brei.md)
  : Layer on top of the engine
  
[Programs](./program.md)
  : State machine around the tasks

[Template Strings](./template_strings.md)
  : Run-time string interpolation

[Compatibility](./compat.md)
  : Ensuring compatibility with the older Python implementation
  
[Coverage](./coverage.md)
  : Test coverage report
