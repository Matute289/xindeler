# VPS Deploy Review — server-cli Build & Push

> **Contexto:** El workflow `.github/workflows/publish-docker.yml` falla en cada push a main porque intenta buildear y pushear a GHCR (GitHub Container Registry). GHCR ya no se usa — el deployment se hace a través de un script local en el servidor VPS.

## Información del servidor

**Toda la información del servidor VPS, scripts de deployment, credenciales y configuración está en:**
```
/Users/mgrinberg/MyServerVPS
```

Antes de ejecutar esta tarea, leer el contenido de ese directorio para entender:
- Cómo se buildea y deploya `server-cli` al VPS
- Qué scripts existen
- Si hay algún workflow de CI que deba actualizarse o desactivarse

## Problema a resolver

El workflow `publish-docker.yml` falla en cada push a `main` porque:
1. Intenta buildear el binario `veloren-server-cli` y crear una imagen Docker
2. Intenta pushear la imagen a `ghcr.io/mgrinberg/veloren/server-cli`
3. GHCR ya no se usa — hay un script en `~/MyServerVPS` que maneja el deployment

## Tareas

- [ ] Revisar `/Users/mgrinberg/MyServerVPS` para entender el mecanismo actual de deploy
- [ ] Evaluar opciones para `.github/workflows/publish-docker.yml`:
  - Opción A: Desactivar el workflow completamente (`enabled: false` o borrar el archivo)
  - Opción B: Reemplazarlo por un workflow que llame al script del VPS via SSH
  - Opción C: Dejar el workflow pero cambiar el trigger para que no corra automáticamente
- [ ] Implementar la solución elegida
- [ ] Verificar que no haya otros workflows que intenten pushear a GHCR
- [ ] Commitear y pushear a `main`

## Nota para el agente remoto

**El agente remoto NO tiene acceso a `/Users/mgrinberg/MyServerVPS`** — ese path es local en la máquina del usuario. La información del servidor debe ser provista en el prompt o el agente debe consultar al usuario antes de actuar. La opción más segura es **desactivar el workflow** y dejar que el usuario configure el deployment desde su máquina local.
