# Tamatoa

<img src="./img/DALL-E-SAC.png" width="100" height="100">

Tamatoa is a multi-platform application designed to quickly and efficiently gather evidence from endpoints.

It has been designed to be deployed to customer systems as part of an incident response scenario.

The initial release will provide support for Windows, Mac and Linux Operating systems on an x86_64 architecture, with a view to release for M1 Macs in the near future.

The major inspiration for this is the [CyLR project](https://github.com/orlikoski/CyLR/), and a want to learn Rust.

My aim for this project is to replicate all the major functionality of the latest CyLR release, and take it further from there, where possible.

## Installation

Grab the latest release from the release page, and deploy to a system you want to collect artifacts.

## Usage

Ideally run this as a superuser (though it will execute as standard user, it won't collect all the telemetry it would normally be able to).

Run it with no commands to see the options available: 

```
Tamatoa Version 0.3.0


Usage: tamatoa [Options]... [Files]...

The tamatoa tool collects forensic artifacts from hosts with NTFS file systems quickly, securely and minimizes impact to the host.

The available options are:

-v              Set verbosity of the console log. By default the console only shows information or greater events and the file log shows all entries. Disabled when `-q` is used.
         Usage: -v [trace, info, warn, error, none]
-d              Same as '-c' but will collect default paths included in tamatoa in addition to those specified in the provided config file.
        Usage: -d <path to config file>
-of             Defines the name of the zip archive will be created. Defaults to host machine's name.
        Usage: -of <archive name>
-c              Optional argument to provide custom list of artifact files and directories (one entry per line). NOTE: Please see CUSTOM_PATH_TEMPLATE.txt for sample.
        Usage: -c <path to config file>
-od             Defines the directory that the zip archive will be created in. Defaults to current working directory.
        Usage: -od <directory path>
--usnjrnl               Enables collecting $UsnJrnl
-hf             Generate a hashes for each of the files being collected - uses SHA256 algorithm to generate a hash of the files. WARNING: will increase execution time.
-q              Disables logging to the console and file.
         Usage: -q
```

Execution with any arguments set will cause it to begin collection of data.

## Image Attribution

DALL-E OpenAI. 