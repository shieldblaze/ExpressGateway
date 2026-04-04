/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.common.utils;

import lombok.extern.slf4j.Slf4j;

import java.io.Closeable;
import java.io.IOException;
import java.nio.file.FileSystems;
import java.nio.file.Path;
import java.nio.file.StandardWatchEventKinds;
import java.nio.file.WatchEvent;
import java.nio.file.WatchKey;
import java.nio.file.WatchService;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CopyOnWriteArraySet;
import java.util.function.Consumer;

/**
 * Utility class that monitors files for changes using the JDK {@link WatchService}.
 *
 * <p>Usage:</p>
 * <pre>
 *   FileWatcher watcher = new FileWatcher();
 *   watcher.watch(certFile, path -> reloadCertificate(path));
 *   watcher.start();
 *   // ...
 *   watcher.close();
 * </pre>
 *
 * <p>The watcher runs on a dedicated daemon thread and detects file modifications.
 * The callback is invoked on the watcher thread; heavy operations should be
 * dispatched elsewhere.</p>
 *
 * <p>Thread-safe: callbacks can be registered/unregistered from any thread.</p>
 */
@Slf4j
public final class FileWatcher implements Closeable {

    private final WatchService watchService;
    private final Map<Path, Set<Consumer<Path>>> callbacks = new ConcurrentHashMap<>();
    private final Map<Path, WatchKey> watchKeys = new ConcurrentHashMap<>();
    private volatile boolean running;
    private Thread watchThread;

    public FileWatcher() throws IOException {
        this.watchService = FileSystems.getDefault().newWatchService();
    }

    /**
     * Register a file to watch. The callback is invoked when the file is modified.
     *
     * @param file     the file to watch
     * @param callback consumer invoked with the file path on change
     */
    public void watch(Path file, Consumer<Path> callback) throws IOException {
        Objects.requireNonNull(file, "file");
        Objects.requireNonNull(callback, "callback");

        // MED-10: Use the full resolved path as the callback map key, not just the
        // filename. Using filename alone causes collisions when watching files with
        // the same name in different directories (e.g., /etc/certs/cert.pem and
        // /etc/certs-staging/cert.pem would share callbacks).
        Path resolved = file.toAbsolutePath().normalize();
        Path dir = resolved.getParent();
        if (dir == null) {
            throw new IllegalArgumentException("Cannot watch file without a parent directory: " + file);
        }

        callbacks.computeIfAbsent(resolved, k -> new CopyOnWriteArraySet<>()).add(callback);

        // Register the parent directory if not already registered
        if (!watchKeys.containsKey(dir)) {
            WatchKey key = dir.register(watchService, StandardWatchEventKinds.ENTRY_MODIFY);
            watchKeys.put(dir, key);
            log.info("FileWatcher: registered directory {} for monitoring", dir);
        }
    }

    /**
     * Start the file watcher thread.
     */
    public void start() {
        if (running) {
            return;
        }
        running = true;
        watchThread = new Thread(this::watchLoop, "file-watcher");
        watchThread.setDaemon(true);
        watchThread.start();
        log.info("FileWatcher started");
    }

    private void watchLoop() {
        while (running) {
            try {
                WatchKey key = watchService.take();

                for (WatchEvent<?> event : key.pollEvents()) {
                    if (event.kind() == StandardWatchEventKinds.OVERFLOW) {
                        continue;
                    }

                    @SuppressWarnings("unchecked")
                    WatchEvent<Path> pathEvent = (WatchEvent<Path>) event;
                    Path changedFileName = pathEvent.context();

                    // MED-10: Resolve to the full absolute path to match the key
                    // used in the callbacks map (registered with toAbsolutePath().normalize()).
                    Path watchedDir = findWatchedDir(key);
                    Path fullPath = watchedDir != null
                            ? watchedDir.resolve(changedFileName).toAbsolutePath().normalize()
                            : changedFileName;

                    Set<Consumer<Path>> fileCallbacks = callbacks.get(fullPath);
                    if (fileCallbacks != null) {
                        log.info("FileWatcher: detected change in {}", fullPath);

                        for (Consumer<Path> callback : fileCallbacks) {
                            try {
                                callback.accept(fullPath);
                            } catch (Exception ex) {
                                log.error("FileWatcher: callback failed for {}", fullPath, ex);
                            }
                        }
                    }
                }

                if (!key.reset()) {
                    log.warn("FileWatcher: watch key invalidated, some files may no longer be monitored");
                }
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            } catch (Exception ex) {
                log.error("FileWatcher: error in watch loop", ex);
            }
        }
    }

    private Path findWatchedDir(WatchKey key) {
        for (Map.Entry<Path, WatchKey> entry : watchKeys.entrySet()) {
            if (entry.getValue().equals(key)) {
                return entry.getKey();
            }
        }
        return null;
    }

    @Override
    public void close() throws IOException {
        running = false;
        if (watchThread != null) {
            watchThread.interrupt();
        }
        for (WatchKey key : watchKeys.values()) {
            key.cancel();
        }
        watchService.close();
        log.info("FileWatcher stopped");
    }
}
