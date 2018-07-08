## docker-guard

This project aims to provide a more secure `docker.sock` that can be used
without fear inside of untrusted containers. Currently it only allows the
client to perform `docker ps` and `docker inspect`.

In `docker inspect` all environment variables are filtered out except the
ones that are white-listed. This ensures that no secrets that are set via
environment variables are exposed to untrusted containers.

## Usage

This project is not ready yet. Usage will be updated on the first release.

## License

MIT
