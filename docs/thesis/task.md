# Context
I am trying to write my Mater's thesis and want to work on the part of my contribution/implementation. 
This part focuses on the architecture of the solution, how it has being validated, results, with 
a final section as a summary with future perspectives.

The contribution part mainly focuses on what we added to the existing tool, it should be added as a note 
in the final document to mention that in the introduction of the contribution part.

I will be working on the various parts mentioned above in separate markdown files. The markdown files 
would be then given to an agent to aggregate all the results and generate the latex code, in the 
context of an existing code base for the previous two parts of my thesis (background and state of the 
art).

## Handling References
For any references during the creation of the markdown files they should be added at the end 
of the final generated file. It is the job of the integrator agent to cross reference them to actual 
references given the context it has of the thesis code base, and flag any ones that don't map 
cleanly.

# Target
Now I want to work on the architecture chapter, which is why I am currently in the codebase for my solution 
the ./overview.md contains information about the approach with what I want to explain about it. The 
goal of the architecture chapter is to explain the solution at a high level and to give a global 
picture of it. Since my work is mainly focused on the attribution, which is what the whole theme 
revolves around, I need to get into some detail about my approach. However specific code is not that 
much relevant in my case.

# Task
I want the agent to help me in drafting the markdown file for the architecture chapter given the 
context it has. The document includes any visuals required. The visuals should be in an appropriate 
format for the thesis.
1. I want you to start by proposing a good method to handle visual creation, would mermaid work? 
an MCP server? Do you want to give me a prompt I can plug into something like eraser?.
2. I want you to fill in missing parts indicted with in {ALL CAPS COMMENTS} withing the ./overview.md 
document, without changing the core of the current information
3. I want you to tidy up the current overview, generating an architecture document with the full: 
explanation + visuals.

# Resources
You can find my current thesis in ./main.txt

# Diagrams
For the diagrams that are not plots provided by me (meaning that are Mermaid diagram in the markdown) 
The graphviz scripts should have then named appropriate and prefixed by contribution_. For the 
validation chapter if the diagrams refer to experiments then they have to have the name 
contribution_{experiment}.

For the final document you should add a list of the diagram names and add their path as 
./diagrams/{generated name}, along with any plots used or mentioned in the markdown.

The diagram scripts should generate png format

# Notes
For any things in the output markdown that need some looking into or modification, make sure to add them 
into a notes section. For example if a result is absurdly wrong add it there and say that 
it needs fixing later on.
