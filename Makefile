.PHONY: dev prod build clean

dev:
	docker-compose -f docker-compose.yml -f docker-compose.dev.yml up

dev-daemon:
	docker-compose -f docker-compose.yml -f docker-compose.dev.yml up -d

dev-build:
	docker-compose -f docker-compose.yml -f docker-compose.dev.yml up --build

prod:
	docker-compose up

build:
	docker-compose build

clean:
	docker-compose down -v
	docker system prune -f
