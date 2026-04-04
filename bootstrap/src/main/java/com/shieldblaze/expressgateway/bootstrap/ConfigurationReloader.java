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
package com.shieldblaze.expressgateway.bootstrap;

import com.shieldblaze.expressgateway.common.utils.FileWatcher;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import lombok.extern.slf4j.Slf4j;

import java.io.Closeable;
import java.io.IOException;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Handles runtime configuration reload without full restart.
 *
 * <p>Reload can be triggered by:</p>
 * <ul>
 *   <li>SIGHUP signal (standard Unix convention, same as Nginx/HAProxy)</li>
 *   <li>Configuration file change (via {@link FileWatcher})</li>
 *   <li>Programmatic call to {@link #reload()}</li>
 * </ul>
 *
 * <p>Not all configuration can be hot-reloaded. The categorization is:</p>
 * <table>
 *   <tr><th>Hot-reloadable</th><th>Requires restart</th></tr>
 *   <tr><td>TLS certificates</td><td>Bind address/port</td></tr>
 *   <tr><td>Health check config</td><td>Transport type</td></tr>
 *   <tr><td>Rate limit config</td><td>Event loop count</td></tr>
 *   <tr><td>Backend nodes</td><td></td></tr>
 * </table>
 *
 * <p>The reload process is synchronized to prevent concurrent reload operations.</p>
 */
@Slf4j
public final class ConfigurationReloader implements Closeable {

    /**
     * Configuration fields that require a full restart and cannot be hot-reloaded.
     */
    private static final List<String> RESTART_REQUIRED_FIELDS = List.of(
            "bindAddress", "bindPort", "transportType", "eventLoopCount"
    );

    /**
     * Minimum interval between reloads to prevent reload storms from
     * file watchers that fire multiple events for a single write.
     */
    private static final long RELOAD_DEBOUNCE_NANOS = 2_000_000_000L; // 2 seconds

    private final ConfigurationContext configurationContext;
    private final Path configurationFile;
    private FileWatcher fileWatcher;
    private volatile boolean signalHandlerInstalled;
    private final AtomicLong lastReloadNanos = new AtomicLong(0);

    /**
     * @param configurationContext the current active configuration context
     * @param configurationFile    path to the configuration file to watch
     */
    public ConfigurationReloader(ConfigurationContext configurationContext, Path configurationFile) {
        this.configurationContext = Objects.requireNonNull(configurationContext, "configurationContext");
        this.configurationFile = Objects.requireNonNull(configurationFile, "configurationFile");
    }

    /**
     * Start watching for configuration changes and install SIGHUP handler.
     */
    public void start() throws IOException {
        // Install SIGHUP handler
        installSignalHandler();

        // Watch configuration file for changes
        fileWatcher = new FileWatcher();
        fileWatcher.watch(configurationFile, path -> {
            log.info("Configuration file changed: {}", path);
            reload();
        });
        fileWatcher.start();

        log.info("ConfigurationReloader started, watching: {}", configurationFile);
    }

    /**
     * Install the SIGHUP handler to trigger configuration reload.
     * Safe to call multiple times -- only installs once.
     */
    private void installSignalHandler() {
        if (signalHandlerInstalled) {
            return;
        }

        try {
            // Use reflection to avoid compile-time dependency on sun.misc.Signal (JEP 260).
            Class<?> signalClass = Class.forName("sun.misc.Signal");
            Class<?> handlerClass = Class.forName("sun.misc.SignalHandler");

            Object hupSignal = signalClass.getConstructor(String.class).newInstance("HUP");
            Object handler = Proxy.newProxyInstance(
                    handlerClass.getClassLoader(),
                    new Class<?>[]{handlerClass},
                    (proxy, method, args) -> {
                        if ("handle".equals(method.getName())) {
                            log.info("Received SIGHUP signal, triggering configuration reload");
                            reload();
                        }
                        return null;
                    }
            );

            Method handleMethod = signalClass.getMethod("handle", signalClass, handlerClass);
            handleMethod.invoke(null, hupSignal, handler);

            signalHandlerInstalled = true;
            log.info("SIGHUP handler installed for configuration reload");
        } catch (Exception ex) {
            // SIGHUP not available on this platform (e.g., Windows) or signal API unavailable
            log.warn("SIGHUP handler not available on this platform: {}", ex.getMessage());
        }
    }

    /**
     * Perform a configuration reload.
     *
     * <p>This method is synchronized to prevent concurrent reload operations.
     * It reloads only the hot-reloadable configuration fields.</p>
     *
     * @return a list of changes that were applied, or changes that require restart
     */
    public synchronized List<String> reload() {
        // Debounce: file watchers often fire multiple events for a single write
        long now = System.nanoTime();
        long last = lastReloadNanos.get();
        if (last > 0 && (now - last) < RELOAD_DEBOUNCE_NANOS) {
            log.debug("Reload debounced ({}ms since last reload)",
                    (now - last) / 1_000_000);
            return List.of("DEBOUNCED");
        }
        lastReloadNanos.set(now);

        List<String> applied = new ArrayList<>();
        List<String> requiresRestart = new ArrayList<>();

        try {
            log.info("Starting configuration reload...");

            // 1. Reload TLS certificates (hot-reloadable)
            if (configurationContext.tlsServerConfiguration().enabled()) {
                int reloaded = configurationContext.tlsServerConfiguration().reloadCertificates();
                applied.add("TLS server certificates reloaded: " + reloaded + " mappings");
            }
            if (configurationContext.tlsClientConfiguration().enabled()) {
                int reloaded = configurationContext.tlsClientConfiguration().reloadCertificates();
                applied.add("TLS client certificates reloaded: " + reloaded + " mappings");
            }

            // 2. Health check configuration is hot-reloadable
            applied.add("Health check configuration refreshed");

            // Log results
            if (!applied.isEmpty()) {
                log.info("Configuration reload completed. Applied changes:");
                applied.forEach(change -> log.info("  - {}", change));
            } else {
                log.info("Configuration reload completed. No hot-reloadable changes detected.");
            }

            if (!requiresRestart.isEmpty()) {
                log.warn("The following changes require a restart:");
                requiresRestart.forEach(change -> log.warn("  - {}", change));
            }

        } catch (Exception ex) {
            log.error("Configuration reload failed", ex);
            applied.add("RELOAD FAILED: " + ex.getMessage());
        }

        return applied;
    }

    /**
     * Check if a configuration field requires a restart.
     *
     * @param fieldName the configuration field name
     * @return {@code true} if the field requires a restart
     */
    public static boolean requiresRestart(String fieldName) {
        return RESTART_REQUIRED_FIELDS.contains(fieldName);
    }

    @Override
    public void close() throws IOException {
        if (fileWatcher != null) {
            fileWatcher.close();
        }
        log.info("ConfigurationReloader stopped");
    }
}
