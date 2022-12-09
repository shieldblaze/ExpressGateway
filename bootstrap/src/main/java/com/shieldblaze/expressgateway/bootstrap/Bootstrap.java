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
import com.shieldblaze.expressgateway.restapi.RestApi;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.File;
import java.nio.file.Path;

import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnv;

/**
 * This class initializes and boots up the ExpressGateway.
 */
public final class Bootstrap {
    private static final Logger logger = LogManager.getLogger(Bootstrap.class);

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
        loadApplicationFile();
    }

    private static void loadApplicationFile() throws Exception {
        try {
            String configurationDirectory = getPropertyOrEnv("CONFIGURATION_DIRECTORY", "/etc/expressgateway/conf.d");
            logger.info("Configuration directory: {}", configurationDirectory);

            Path configurationFile = Path.of(configurationDirectory + File.separator + getPropertyOrEnv("CONFIGURATION_FILE_NAME", "configuration.json"));
            logger.info("Loading ExpressGateway Configuration file: {}", configurationFile.toAbsolutePath());

            ObjectMapper objectMapper = new ObjectMapper();
            ExpressGateway.setInstance(objectMapper.readValue(configurationFile.toFile(), ExpressGateway.class));

            logger.info("[CONFIGURATION] RunningMode: {}", ExpressGateway.getInstance().runningMode());
            logger.info("[CONFIGURATION] ClusterID: {}", ExpressGateway.getInstance().clusterID());
            logger.info("[CONFIGURATION] Environment: {}", ExpressGateway.getInstance().environment());
            logger.info("[CONFIGURATION] Rest-API: {}", ExpressGateway.getInstance().restApi());
            logger.info("[CONFIGURATION] ZooKeeper: {}", ExpressGateway.getInstance().zooKeeper());
            logger.info("[CONFIGURATION] ServiceDiscovery: {}", ExpressGateway.getInstance().serviceDiscovery());
            logger.info("[CONFIGURATION] LoadBalancerTLS: {}", ExpressGateway.getInstance().loadBalancerTLS());

            // Initialize
            Curator.init();
            assert Curator.isInitialized().get() : "Failed to initialize ZooKeeper";
            assert CertificateManager.isInitialized().get() : "Failed to initialize CertificateManager";

            RestApi.start();
        } catch (Exception ex) {
            logger.error("Failed to Bootstrap", ex);
            throw ex;
        }
    }

    public static void shutdown() {
        // Shutdown the Rest API Server
        RestApi.stop();
    }
}
