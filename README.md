## docker-guard

This project aims to provide a restricted `docker.sock` that can be used
without fear inside of untrusted containers. This ensures that no secrets
are exposed to the containers that have access to `docker.sock`.

Client is restricted to the following commands (everything else is blocked):

`docker ps` - It only shows the ID, status, and when container is created.
Everything else is filtered out.

`docker inspect <id>` - It only shows: ID, name, state, network settings,
and white-listed environment variables. Everything else is filtered out.

`docker version` - This commands is not filtered since there are no critical
information.

`docker info` - This shows basic information about the docker daemon, such as:
how many containers exists, how many are running, memory limit, etc. You can
find a complete list in `info` function of [filters.rs].

## Usage

This project is not ready yet. Usage will be updated on the first release.

## License

MIT


[filters.rs]: src/filters.rs
