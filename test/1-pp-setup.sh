set -e
set -x

sed -i '' 's/^DOCKER_COMPOSE_FILE=.*/DOCKER_COMPOSE_FILE=docker-compose.yml/' .env

source .env

docker compose -f ${DOCKER_COMPOSE_FILE} down --remove-orphans
make run