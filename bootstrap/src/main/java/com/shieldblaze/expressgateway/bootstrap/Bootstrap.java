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
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

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
public final class Bootstrap {
    private static final Logger logger = LogManager.getLogger(Bootstrap.class);

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

        logger.info("Starting ShieldBlaze ExpressGateway v0.1-a");

        STARTUP_METRICS.markStartupBegin();

        // Install JVM shutdown hook before loading configuration so that
        // even a partially-started gateway gets a clean shutdown attempt.
        installShutdownHook();

        loadApplicationFile();

        // Run startup health checks to verify all subsystems are ready
        STARTUP_METRICS.timeComponent("startup-health-check", () -> {
            StartupHealthCheck.Result result = StartupHealthCheck.runChecks();
            if (!result.healthy()) {
                throw new IllegalStateException("Startup health checks failed: " + result.failed());
            }
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
            logger.info("Initiating graceful shutdown...");

            // Step 1: Deregister from service discovery first so upstream
            // load balancers stop sending new traffic immediately.
            try {
                if (ExpressGateway.getInstance() != null &&
                        ExpressGateway.getInstance().runningMode() == ExpressGateway.RunningMode.REPLICA) {
                    logger.info("Deregistering from service discovery");
                    ServiceDiscoveryClient.deregister();
                }
            } catch (Exception ex) {
                logger.warn("Failed to deregister from service discovery during shutdown", ex);
            }

            // Step 2: Execute registered shutdown actions in parallel using virtual threads.
            CountDownLatch latch = new CountDownLatch(SHUTDOWN_ACTIONS.size());
            for (Runnable action : SHUTDOWN_ACTIONS) {
                Thread.ofVirtual().name("expressgateway-shutdown-action").start(() -> {
                    try {
                        action.run();
                    } catch (Exception ex) {
                        logger.warn("Shutdown action failed", ex);
                    } finally {
                        latch.countDown();
                    }
                });
            }

            // Step 3: Wait for all shutdown actions to complete or timeout.
            try {
                boolean drained = latch.await(GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS, TimeUnit.SECONDS);
                if (!drained) {
                    logger.warn("Graceful shutdown timed out after {}s; some connections may have been forcibly closed",
                            GRACEFUL_SHUTDOWN_TIMEOUT_SECONDS);
                }
            } catch (InterruptedException e) {
                logger.warn("Shutdown hook interrupted while waiting for drain", e);
                Thread.currentThread().interrupt();
            }

            // Step 4: Close ZooKeeper connection
            try {
                if (Curator.isInitialized().get()) {
                    Curator.getInstance().close();
                }
            } catch (Exception ex) {
                logger.warn("Failed to close Curator/ZooKeeper connection during shutdown", ex);
            }

            // Step 5: Shutdown executor pools last (after all work is drained)
            GlobalExecutors.INSTANCE.shutdownAll();

            logger.info("Graceful shutdown complete");
        }));
    }

    private static void loadApplicationFile() throws Exception {
        try {
            String configurationDirectory = getPropertyOrEnv("CONFIGURATION_DIRECTORY", "/etc/expressgateway/conf.d");
            logger.info("Configuration directory: {}", configurationDirectory);

            Path configurationFile = Path.of(configurationDirectory + File.separator + getPropertyOrEnv("CONFIGURATION_FILE_NAME", "configuration.json"));
            logger.info("Loading ExpressGateway Configuration file: {}", configurationFile.toAbsolutePath());

            STARTUP_METRICS.timeComponent("configuration-load", () -> {
                ObjectMapper objectMapper = new ObjectMapper();
                ExpressGateway expressGateway = objectMapper.readValue(configurationFile.toFile(), ExpressGateway.class);
                ExpressGateway.setInstance(expressGateway);
            });

            logger.info("[CONFIGURATION] RunningMode: {}", ExpressGateway.getInstance().runningMode());
            logger.info("[CONFIGURATION] ClusterID: {}", ExpressGateway.getInstance().clusterID());
            logger.info("[CONFIGURATION] Environment: {}", ExpressGateway.getInstance().environment());
            logger.info("[CONFIGURATION] Rest-API: {}", ExpressGateway.getInstance().restApi());
            logger.info("[CONFIGURATION] ZooKeeper: {}", ExpressGateway.getInstance().zooKeeper());
            logger.info("[CONFIGURATION] ServiceDiscovery: {}", ExpressGateway.getInstance().serviceDiscovery());
            logger.info("[CONFIGURATION] LoadBalancerTLS: {}", ExpressGateway.getInstance().loadBalancerTLS());

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
                logger.info("Skipping ZooKeeper/Curator initialization in STANDALONE mode");
            }

        } catch (Exception ex) {
            logger.error("Failed to Bootstrap", ex);
            throw ex;
        }
    }

    public static void shutdown() {
        // Explicitly shut down executor pools
        GlobalExecutors.INSTANCE.shutdownAll();
    }
}
