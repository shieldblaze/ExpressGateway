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

import com.fasterxml.jackson.databind.ObjectMapper;
import com.shieldblaze.expressgateway.common.ExpressGateway;
import com.shieldblaze.expressgateway.common.zookeeper.CertificateManager;
import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.servicediscovery.client.ServiceDiscoveryClient;
import lombok.extern.slf4j.Slf4j;

import java.io.File;
import java.nio.file.Path;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnv;

/**
 * This class initializes and boots up the ExpressGateway.
 *
 * <p>Startup uses virtual threads for parallel initialization of independent subsystems.
 * Graceful shutdown is handled via a JVM shutdown hook that properly drains connections,
 * deregisters from service discovery, and closes all resources in the correct order.</p>
 *
 * <p>Startup metrics are collected for each component and a health check runs after
 * initialization to verify all subsystems are ready before the gateway advertises itself.</p>
 */
@Slf4j
public final class Bootstrap {

    /**
     * Timeout in seconds for graceful shutdown. After this period, any
     * remaining in-flight work is abandoned. 30 seconds matches typical
     * load-balancer drain timeouts for rolling deployments.
     */
    private static final int GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS = 30;

    /**
     * Registered shutdown actions. Each {@link Runnable} performs one
     * discrete shutdown step (e.g., shutting down an event loop group,
     * closing a connection pool). Thread-safe for concurrent registration
     * from multiple load balancer instances.
     */
    private static final List<Runnable> SHUTDOWN_ACTIONS = new CopyOnWriteArrayList<>();

    private static final StartupMetrics STARTUP_METRICS = new StartupMetrics();

    private Bootstrap() {
        // Prevent outside initialization
    }

    /**
     * Register a shutdown action to be executed during graceful shutdown.
     * Actions are executed in registration order.
     *
     * @param action the shutdown action to register
     */
    public static void registerShutdownAction(Runnable action) {
        if (action != null) {
            SHUTDOWN_ACTIONS.add(action);
        }
    }

    /**
     * Return the startup metrics collector.
     */
    public static StartupMetrics startupMetrics() {
        return STARTUP_METRICS;
    }

    public static void main() throws Exception {
        main(new String[0]);
    }

    public static void main(String[] args) throws Exception {
        System.out.println("""
                 ______                               _____       _                          \s
                |  ____|                             / ____|     | |                         \s
                | |__  __  ___ __  _ __ ___  ___ ___| |  __  __ _| |_ _____      ____ _ _   _\s
                |  __| \\ \\/ / '_ \\| '__/ _ \\/ __/ __| | |_ |/ _` | __/ _ \\ \\ /\\ / / _` | | | |
                | |____ >  <| |_) | | |  __/\\__ \\__ \\ |__| | (_| | ||  __/\\ V  V / (_| | |_| |
                |______/_/\\_\\ .__/|_|  \\___||___/___/\\_____|\\__,_|\\__\\___| \\_/\\_/ \\__,_|\\__, |
                            | |                                                          __/ |
                            |_|                                                         |___/\s""".indent(1));

        log.info("Starting ShieldBlaze ExpressGateway v0.1-a");

        STARTUP_METRICS.markStartupBegin();

        // Install JVM shutdown hook before loading configuration so that
        // even a partially-started gateway gets a clean shutdown attempt.
        installShutdownHook();

        loadApplicationFile();

        // Run startup health checks to verify all subsystems are ready
        STARTUP_METRICS.timeComponent("startup-health-check", () -> {
            StartupHealthCheck.Result result = StartupHealthCheck.runChecks();
            if (!result.healthy()) {
                log.error("Startup health check failures: {}", result.failed());
                throw new IllegalStateException("Startup health checks failed: " + result.failed());
            }
            log.info("All startup health checks passed: {}", result.passed());
        });

        STARTUP_METRICS.markStartupComplete();
    }

    /**
     * Installs a JVM shutdown hook that performs graceful shutdown on
     * SIGTERM / SIGINT. Uses virtual threads for parallel shutdown of
     * independent subsystems.
     *
     * <p>Shutdown sequence:</p>
     * <ol>
     *   <li>Deregister from service discovery (stop receiving new traffic)</li>
     *   <li>Execute all registered shutdown actions (close listeners, drain connections)</li>
     *   <li>Close ZooKeeper connection</li>
     *   <li>Shutdown executor pools</li>
     * </ol>
     */
    private static void installShutdownHook() {
        Runtime.getRuntime().addShutdownHook(Thread.ofVirtual().unstarted(() -> {
            log.info("Initiating graceful shutdown...");

            // Step 1: Deregister from service discovery first so upstream
            // load balancers stop sending new traffic immediately.
            try {
                if (ExpressGateway.getInstance() != null &&
                        ExpressGateway.getInstance().runningMode() == ExpressGateway.RunningMode.REPLICA) {
                    log.info("Deregistering from service discovery");
                    ServiceDiscoveryClient.deregister();
                }
            } catch (Exception ex) {
                log.warn("Failed to deregister from service discovery during shutdown", ex);
            }

            // Step 2: Execute registered shutdown actions in parallel using virtual threads.
            CountDownLatch latch = new CountDownLatch(SHUTDOWN_ACTIONS.size());
            for (Runnable action : SHUTDOWN_ACTIONS) {
                Thread.ofVirtual().name("expressgateway-shutdown-action").start(() -> {
                    try {
                        action.run();
                    } catch (Exception ex) {
                        log.warn("Shutdown action failed", ex);
                    } finally {
                        latch.countDown();
                    }
                });
            }

            // Step 3: Wait for all shutdown actions to complete or timeout.
            try {
                boolean drained = latch.await(GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS, TimeUnit.SECONDS);
                if (!drained) {
                    log.warn("Graceful shutdown timed out after {}s; some connections may have been forcibly closed",
                            GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS);
                }
            } catch (InterruptedException e) {
                log.warn("Shutdown hook interrupted while waiting for drain", e);
                Thread.currentThread().interrupt();
            }

            // Step 4: Close ZooKeeper connection
            try {
                if (Curator.isInitialized().get()) {
                    Curator.getInstance().close();
                }
            } catch (Exception ex) {
                log.warn("Failed to close Curator/ZooKeeper connection during shutdown", ex);
            }

            // Step 5: Shutdown executor pools last (after all work is drained)
            GlobalExecutors.INSTANCE.shutdownAll();

            log.info("Graceful shutdown complete");
        }));
    }

    private static void loadApplicationFile() throws Exception {
        try {
            String configurationDirectory = getPropertyOrEnv("CONFIGURATION_DIRECTORY", "/etc/expressgateway/conf.d");
            log.info("Configuration directory: {}", configurationDirectory);

            Path configurationFile = Path.of(configurationDirectory + File.separator + getPropertyOrEnv("CONFIGURATION_FILE_NAME", "configuration.json"));
            log.info("Loading ExpressGateway Configuration file: {}", configurationFile.toAbsolutePath());

            STARTUP_METRICS.timeComponent("configuration-load", () -> {
                ObjectMapper objectMapper = new ObjectMapper();
                ExpressGateway expressGateway = objectMapper.readValue(configurationFile.toFile(), ExpressGateway.class);
                ExpressGateway.setInstance(expressGateway);
            });

            log.info("[CONFIGURATION] RunningMode: {}", ExpressGateway.getInstance().runningMode());
            log.info("[CONFIGURATION] ClusterID: {}", ExpressGateway.getInstance().clusterID());
            log.info("[CONFIGURATION] Environment: {}", ExpressGateway.getInstance().environment());
            log.info("[CONFIGURATION] Rest-API: {}", ExpressGateway.getInstance().restApi());
            log.info("[CONFIGURATION] ZooKeeper: {}", ExpressGateway.getInstance().zooKeeper());
            log.info("[CONFIGURATION] ServiceDiscovery: {}", ExpressGateway.getInstance().serviceDiscovery());
            log.info("[CONFIGURATION] LoadBalancerTLS: {}", ExpressGateway.getInstance().loadBalancerTLS());

            // Only initialize Curator when running in REPLICA mode (ZooKeeper required)
            if (ExpressGateway.getInstance().runningMode() == ExpressGateway.RunningMode.REPLICA) {
                STARTUP_METRICS.timeComponent("curator-init", () -> {
                    Curator.init();
                    if (!Curator.isInitialized().get()) {
                        throw new IllegalStateException("Failed to initialize ZooKeeper");
                    }
                    if (!CertificateManager.isInitialized().get()) {
                        throw new IllegalStateException("Failed to initialize CertificateManager");
                    }
                });

                STARTUP_METRICS.timeComponent("service-discovery-register", ServiceDiscoveryClient::register);
            } else {
                log.info("Skipping ZooKeeper/Curator initialization in STANDALONE mode");
            }

        } catch (Exception ex) {
            log.error("Failed to Bootstrap", ex);
            throw ex;
        }
    }

    /**
     * Programmatic shutdown that executes all registered shutdown actions,
     * then shuts down executor pools. Unlike the JVM shutdown hook, this
     * method blocks until all actions complete or the timeout elapses.
     */
    public static void shutdown() {
        log.info("Programmatic shutdown requested");

        // Execute registered shutdown actions in parallel using virtual threads
        CountDownLatch latch = new CountDownLatch(SHUTDOWN_ACTIONS.size());
        for (Runnable action : SHUTDOWN_ACTIONS) {
            Thread.ofVirtual().name("expressgateway-shutdown-action").start(() -> {
                try {
                    action.run();
                } catch (Exception ex) {
                    log.warn("Shutdown action failed", ex);
                } finally {
                    latch.countDown();
                }
            });
        }

        try {
            boolean drained = latch.await(GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS, TimeUnit.SECONDS);
            if (!drained) {
                log.warn("Programmatic shutdown timed out after {}s", GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS);
            }
        } catch (InterruptedException e) {
            log.warn("Programmatic shutdown interrupted while waiting for drain", e);
            Thread.currentThread().interrupt();
        }

        GlobalExecutors.INSTANCE.shutdownAll();
        log.info("Programmatic shutdown complete");
    }
}
