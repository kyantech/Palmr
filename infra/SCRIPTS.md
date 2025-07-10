## 🚀 Quick Start

Palmr. includes a convenient Makefile to simplify development and deployment tasks:

```bash
# Show all available commands
make help

# Build Docker image with multi-platform support
make build

# Start the application
make start

# View application logs
make logs

# Stop the application
make stop

# Clean up containers and images
make clean

# Update apps version
make update-version
```

### Available Commands:
- `make build` - Build Docker image using the build script in `./infra/`
- `make start` - Start the application using docker-compose
- `make stop` - Stop all running containers
- `make logs` - Show application logs
- `make clean` - Clean up containers and images
- `make shell` - Access the application container shell
- `make update-version` - Update all apps version in package.json

All infrastructure scripts are organized in the `./infra/` directory for better project organization.