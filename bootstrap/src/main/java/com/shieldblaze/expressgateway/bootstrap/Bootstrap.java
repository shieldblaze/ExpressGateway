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

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoEntry;
import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoStore;
import com.shieldblaze.expressgateway.restapi.RestApi;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.security.UnrecoverableKeyException;
import java.security.cert.CertificateException;

import static com.shieldblaze.expressgateway.bootstrap.Utils.checkJsonElementNull;
import static com.shieldblaze.expressgateway.bootstrap.Utils.checkNullEnv;
import static com.shieldblaze.expressgateway.bootstrap.Utils.checkStringNullOrEmptyConf;
import static com.shieldblaze.expressgateway.bootstrap.Utils.checkStringNullOrEmptyEnv;
import static com.shieldblaze.expressgateway.bootstrap.Utils.validateEnforcing;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CLUSTER_ID;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CONFIGURATION_DIRECTORY;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CONFIGURATION_FILE_NAME;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_LOADBALANCER_PASSWORD;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_LOADBALANCER_PKCS12_FILE;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_REST_API_PASSWORD;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_REST_API_PKCS12_FILE;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_ZOOKEEPER_PASSWORD;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_ZOOKEEPER_PKCS12_FILE;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.REST_API_IP_ADDRESS;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.REST_API_PORT;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.RUNNING_MODE;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.ZOOKEEPER_CONNECTION_STRING;
import static com.shieldblaze.expressgateway.common.utils.StringUtil.EMPTY_STRING;
import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnv;
import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnvInt;

/**
 * This class initializes and boots up the ExpressGateway.
 */
public final class Bootstrap {
    private static final Logger logger = LogManager.getLogger(Bootstrap.class);

    private String configurationDirectory;

    private boolean enforceConfigurationFileData = false;
    private RunningMode runningMode;
    private String clusterId;

    private String restApiIpAddress;
    private Integer restApiPort;

    private String zooKeeperConnectionString;

    private String cryptoRestApiPkcs12File;
    private String cryptoRestApiPassword;

    private String cryptoZooKeeperPkcs12File;
    private String cryptoZooKeeperPassword;

    private String cryptoLoadBalancerPkcs12File;
    private String cryptoLoadBalancerPassword;

    public static void main() throws Exception {
        main(new String[0]);
    }

    public static void main(String[] args) throws Exception {
        System.out.println("  ______                               _____       _                           \n" +
                " |  ____|                             / ____|     | |                          \n" +
                " | |__  __  ___ __  _ __ ___  ___ ___| |  __  __ _| |_ _____      ____ _ _   _ \n" +
                " |  __| \\ \\/ / '_ \\| '__/ _ \\/ __/ __| | |_ |/ _` | __/ _ \\ \\ /\\ / / _` | | | |\n" +
                " | |____ >  <| |_) | | |  __/\\__ \\__ \\ |__| | (_| | ||  __/\\ V  V / (_| | |_| |\n" +
                " |______/_/\\_\\ .__/|_|  \\___||___/___/\\_____|\\__,_|\\__\\___| \\_/\\_/ \\__,_|\\__, |\n" +
                "             | |                                                          __/ |\n" +
                "             |_|                                                         |___/ ");

        logger.info("Starting ShieldBlaze ExpressGateway v0.1-a");

        new Bootstrap().loadApplicationFile();
    }

    private void loadApplicationFile() throws Exception {
        try {
            // Initialize Global Variables
            initGlobalVariables();
            logger.info("Configuration directory: {}", configurationDirectory);

            Path configurationFile = Path.of(configurationDirectory + File.separator + getPropertyOrEnv(CONFIGURATION_FILE_NAME.name(), "configuration.json"));

            logger.info("Loading ExpressGateway Configuration file: {}", configurationFile.toAbsolutePath());
            JsonObject globalData = JsonParser.parseString(Files.readString(configurationFile)).getAsJsonObject();

            checkJsonElementNull(globalData.get("enforceConfigurationFileData"), "EnforceConfigurationFileData");
            enforceConfigurationFileData = globalData.get("enforceConfigurationFileData").getAsBoolean();

            if (globalData.get("runningMode") == null) {
                validateEnforcing(enforceConfigurationFileData, "Running Mode");

                checkStringNullOrEmptyEnv(runningMode.name(), "Running Mode");
            } else {
                checkJsonElementNull(globalData.get("runningMode"), "Running Mode");
                runningMode = RunningMode.valueOf(globalData.get("runningMode").getAsString().toUpperCase());
            }

            if (globalData.get("clusterId") == null) {
                validateEnforcing(enforceConfigurationFileData, "Cluster ID");

                checkStringNullOrEmptyEnv(clusterId, "Cluster ID");
            } else {
                checkJsonElementNull(globalData.get("clusterId"), "Cluster ID");
                clusterId = globalData.get("clusterId").getAsString();
            }

            // Initialize Rest-Api
            initRestApiServer(globalData);

            // Initialize ZooKeeper
            initZooKeeper(globalData);

            // Initialize Crypto
            initCrypto(globalData);

            // Initialize Global Properties
            initGlobalProperties();

            // Initialize Rest-Api Server
            initRestApiServer();
        } catch (Exception ex) {
            logger.error("Failed to Bootstrap", ex);
            throw ex;
        }
    }

    /**
     * Initialize global variables
     */
    private void initGlobalVariables() {
        configurationDirectory = getPropertyOrEnv(CONFIGURATION_DIRECTORY.name(), "/etc/expressgateway/conf.d/default/");
        runningMode = RunningMode.valueOf(getPropertyOrEnv(RUNNING_MODE.name(), "STANDALONE").toUpperCase());
        clusterId = getPropertyOrEnv(CLUSTER_ID.name());

        restApiIpAddress = getPropertyOrEnv(REST_API_IP_ADDRESS.name());
        restApiPort = getPropertyOrEnvInt(REST_API_PORT.name());

        zooKeeperConnectionString = getPropertyOrEnv(ZOOKEEPER_CONNECTION_STRING.name(), EMPTY_STRING);

        cryptoRestApiPkcs12File = getPropertyOrEnv(CRYPTO_REST_API_PKCS12_FILE.name(), EMPTY_STRING);
        cryptoRestApiPassword = getPropertyOrEnv(CRYPTO_REST_API_PASSWORD.name(), EMPTY_STRING);

        cryptoZooKeeperPkcs12File = getPropertyOrEnv(CRYPTO_ZOOKEEPER_PKCS12_FILE.name(), EMPTY_STRING);
        cryptoZooKeeperPassword = getPropertyOrEnv(CRYPTO_ZOOKEEPER_PASSWORD.name(), EMPTY_STRING);

        cryptoLoadBalancerPkcs12File = getPropertyOrEnv(CRYPTO_LOADBALANCER_PKCS12_FILE.name(), EMPTY_STRING);
        cryptoLoadBalancerPkcs12File = getPropertyOrEnv(CRYPTO_LOADBALANCER_PASSWORD.name(), EMPTY_STRING);
    }

    private void initRestApiServer(JsonObject globalData) {
        JsonObject restApiData = globalData.getAsJsonObject("rest-api");

        // If Rest-Api data is null then we will load variables by environment variables.
        if (restApiData == null) {
            // If enforcing is set to 'true' then we will throw an error and shutdown.
            validateEnforcing(enforceConfigurationFileData, "Rest-Api");

            // If Rest-Api IP Address environment variable is 'null' or empty then we will throw error and shutdown.
            checkStringNullOrEmptyEnv(restApiIpAddress, "Rest-Api IP Address");

            // If Rest-Api Port environment variable is 'null' or empty then we will throw error and shutdown.
            checkNullEnv(restApiPort, "Rest-Api Port");
        } else {
            // If Rest-Api IP Address data is 'null' or empty then we will throw error and shutdown.
            restApiIpAddress = Utils.checkStringNullOrEmptyConf(restApiData.get("ipAddress"), "Rest-Api IP Address");

            // If Rest-Api Port data is 'null' or empty then we will throw error and shutdown.
            restApiPort = Integer.parseInt(Utils.checkStringNullOrEmptyConf(restApiData.get("port"), "Rest-Api Port"));
        }
    }

    private void initZooKeeper(JsonObject globalData) {
        JsonObject zookeeperData = globalData.getAsJsonObject("zookeeper");

        // If RunningMode is STANDALONE then we don't need to load ZooKeeper ConnectionString
        if (runningMode == RunningMode.STANDALONE) {
            logger.info("Running on STANDALONE mode, No need of ZooKeeper");
            return;
        }

        // If ZooKeeper data is null then we will load variables by environment variables.
        if (zookeeperData == null) {
            // If enforcing is set to 'true' then we will throw an error and shutdown.
            validateEnforcing(enforceConfigurationFileData, "ZooKeeper");

            // If ZooKeeper ConnectionString environment variable is 'null' or empty then we will throw error and shutdown.
            checkStringNullOrEmptyEnv(zooKeeperConnectionString, "ZooKeeper ConnectionString");
        } else {
            // If ZooKeeper ConnectionString data is 'null' or empty then we will throw error and shutdown.
            zooKeeperConnectionString = Utils.checkStringNullOrEmptyConf(zookeeperData.get("connectionString"), "ZooKeeper ConnectionString");
        }
    }

    private void initCrypto(JsonObject globalData) {
        JsonObject cryptoData = globalData.getAsJsonObject("crypto");
        if (cryptoData == null) {
            // If enforcing is set to 'true' then we will throw an error and shutdown.
            validateEnforcing(enforceConfigurationFileData, "Crypto");

            // If Crypto Rest-Api Pkcs12 file environment variable is 'null' or empty then we will throw error and shutdown.
            checkStringNullOrEmptyEnv(cryptoRestApiPkcs12File, "Crypto Rest-Api PKCS12 File");

            // If Crypto Rest-Api Password environment variable is 'null' or empty then we will throw error and shutdown.
            checkStringNullOrEmptyEnv(cryptoRestApiPassword, "Crypto Rest-Api Password");
        } else {
            JsonElement restApiElement = cryptoData.get("rest-api");
            checkJsonElementNull(restApiElement, "Crypto Rest-Api");
            checkStringNullOrEmptyConf(restApiElement.getAsJsonObject().get("pkcs12File"), "Crypto Rest-Api PKCS12 File");
            checkStringNullOrEmptyConf(restApiElement.getAsJsonObject().get("password"), "Crypto Rest-Api Password");

            // If Crypto Rest-Api PKCS12 is empty then the feature is disabled
            if (restApiElement.getAsJsonObject().get("pkcs12File").getAsString().isEmpty()) {
                logger.info("Crypto Rest-Api is disabled");
            } else {
                cryptoRestApiPkcs12File = restApiElement.getAsJsonObject().get("pkcs12File").getAsString();
                cryptoRestApiPassword = restApiElement.getAsJsonObject().get("password").getAsString();
            }

            // -----------------------------------------------------

            JsonElement zooKeeperElement = cryptoData.get("zookeeper");
            checkJsonElementNull(zooKeeperElement, "Crypto ZooKeeper");
            checkStringNullOrEmptyConf(zooKeeperElement.getAsJsonObject().get("pkcs12File"), "Crypto ZooKeeper PKCS12 File");
            checkStringNullOrEmptyConf(zooKeeperElement.getAsJsonObject().get("password"), "Crypto ZooKeeper Password");

            // If Crypto ZooKeeper PKCS12 is empty then the feature is disabled
            if (zooKeeperElement.getAsJsonObject().get("pkcs12File").getAsString().isEmpty()) {
                logger.info("Crypto ZooKeeper is disabled");
            } else {
                cryptoZooKeeperPkcs12File = zooKeeperElement.getAsJsonObject().get("pkcs12File").getAsString();
                cryptoZooKeeperPassword = zooKeeperElement.getAsJsonObject().get("password").getAsString();
            }

            // -----------------------------------------------------

            JsonElement loadBalancerElement = cryptoData.get("loadBalancer");
            checkJsonElementNull(loadBalancerElement, "Crypto LoadBalancer");
            checkStringNullOrEmptyConf(loadBalancerElement.getAsJsonObject().get("pkcs12File"), "Crypto Rest-Api PKCS12 File");
            checkStringNullOrEmptyConf(loadBalancerElement.getAsJsonObject().get("password"), "Crypto Rest-Api Password");

            // If Crypto LoadBalancer PKCS12 is empty then the feature is disabled
            if (loadBalancerElement.getAsJsonObject().get("pkcs12File").getAsString().isEmpty()) {
                logger.info("Crypto LoadBalancer is disabled");
            } else {
                cryptoLoadBalancerPkcs12File = loadBalancerElement.getAsJsonObject().get("pkcs12File").getAsString();
                cryptoLoadBalancerPassword = loadBalancerElement.getAsJsonObject().get("password").getAsString();
            }
        }
    }

    private void initGlobalProperties() {
        System.setProperty(CONFIGURATION_DIRECTORY.name(), configurationDirectory);
        System.setProperty(RUNNING_MODE.name(), runningMode.name());
        System.setProperty(CLUSTER_ID.name(), clusterId);

        System.setProperty(REST_API_IP_ADDRESS.name(), restApiIpAddress);
        System.setProperty(REST_API_PORT.name(), String.valueOf(restApiPort));

        System.setProperty(ZOOKEEPER_CONNECTION_STRING.name(), zooKeeperConnectionString);

        System.setProperty(CRYPTO_REST_API_PKCS12_FILE.name(), cryptoRestApiPkcs12File);
        System.setProperty(CRYPTO_REST_API_PASSWORD.name(), cryptoRestApiPassword);

        System.setProperty(CRYPTO_ZOOKEEPER_PKCS12_FILE.name(), cryptoZooKeeperPkcs12File);
        System.setProperty(CRYPTO_ZOOKEEPER_PASSWORD.name(), cryptoZooKeeperPassword);

        System.setProperty(CRYPTO_LOADBALANCER_PKCS12_FILE.name(), cryptoLoadBalancerPkcs12File);
        System.setProperty(CRYPTO_LOADBALANCER_PASSWORD.name(), cryptoLoadBalancerPassword);
    }

    private void initRestApiServer() throws IOException, UnrecoverableKeyException, CertificateException, KeyStoreException, NoSuchAlgorithmException {
        if (cryptoRestApiPkcs12File.isEmpty()) {
            logger.info("Initializing Rest-Api Server without TLS");
        } else {
            logger.info("Loading Crypto Rest-Api PKCS12 File for TLS support");

            byte[] cryptoRestApiFile = Files.readAllBytes(Path.of(cryptoRestApiPkcs12File).toAbsolutePath());
            try (ByteArrayInputStream inputStream = new ByteArrayInputStream(cryptoRestApiFile)) {
                CryptoEntry cryptoEntry = CryptoStore.fetchPrivateKeyCertificateEntry(inputStream, cryptoRestApiPassword.toCharArray(), "rest-api");
                RestApi.start(cryptoEntry);
            }

            logger.info("Successfully initialized Rest-Api Server with TLS");
        }
    }

    static void shutdown() {
        // Shutdown the Rest API Server
        RestApi.stop();
    }
}
