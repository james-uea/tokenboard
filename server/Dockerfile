FROM node:22-alpine AS frontend

WORKDIR /app/frontend

COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

COPY frontend/ ./
RUN npm run build

FROM node:22-alpine AS runtime

ENV NODE_ENV=production

WORKDIR /app

COPY package.json package-lock.json* ./
RUN npm ci --omit=dev && npm cache clean --force

COPY db.js index.js migrate.js ./
COPY lib ./lib
COPY middleware ./middleware
COPY routes ./routes
COPY --from=frontend /app/public ./public

EXPOSE 3001

# Run migration then start server
CMD ["sh", "-c", "node migrate.js && node index.js"]
