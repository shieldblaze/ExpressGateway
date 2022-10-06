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
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.security.UnrecoverableKeyException;
import java.security.cert.CertificateException;

import static com.shieldblaze.expressgateway.bootstrap.Utils.checkNullEnv;
import static com.shieldblaze.expressgateway.bootstrap.Utils.checkNullOrEmptyConf;
import static com.shieldblaze.expressgateway.bootstrap.Utils.checkNullOrEmptyEnv;
import static com.shieldblaze.expressgateway.bootstrap.Utils.validateEnforce;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CLUSTER_ID;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CONFIGURATION_DIRECTORY;
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
import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnv;
import static com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil.getPropertyOrEnvInt;

/**
 * This class initializes and boots up the ExpressGateway.
 */
public final class Bootstrap {
    private static final Logger logger = LogManager.getLogger(Bootstrap.class);

    private static String configurationDirectory;

    private static boolean enforceConfigurationFileData = false;
    private static RunningMode runningMode;
    private static String clusterId;

    private static String restApiIpAddress;
    private static Integer restApiPort;

    private static String zooKeeperConnectionString;

    private static boolean cryptoRestApiEnabled;
    private static String cryptoRestApiPkcs12File;
    private static String cryptoRestApiPassword;

    private static boolean cryptoZooKeeperEnabled;
    private static String cryptoZooKeeperPkcs12File;
    private static String cryptoZooKeeperPassword;

    private static boolean cryptoLoadBalancerEnabled;
    private static String cryptoLoadBalancerPkcs12File;
    private static String cryptoLoadBalancerPassword;

    public static void main(String[] args) {
        System.out.println("______                               _____       _                           \n" +
                " |  ____|                             / ____|     | |                          \n" +
                " | |__  __  ___ __  _ __ ___  ___ ___| |  __  __ _| |_ _____      ____ _ _   _ \n" +
                " |  __| \\ \\/ / '_ \\| '__/ _ \\/ __/ __| | |_ |/ _` | __/ _ \\ \\ /\\ / / _` | | | |\n" +
                " | |____ >  <| |_) | | |  __/\\__ \\__ \\ |__| | (_| | ||  __/\\ V  V / (_| | |_| |\n" +
                " |______/_/\\_\\ .__/|_|  \\___||___/___/\\_____|\\__,_|\\__\\___| \\_/\\_/ \\__,_|\\__, |\n" +
                "             | |                                                          __/ |\n" +
                "             |_|                                                         |___/ ");

        logger.info("Starting ShieldBlaze ExpressGateway v0.1-a");
        loadApplicationFile();
    }

    private static void loadApplicationFile() {
        try {
            // Initialize Global Variables
            initGlobalVariables();
            logger.info("Configuration directory: {}", configurationDirectory);

            Path configurationFilesPath = Path.of(configurationDirectory + "configuration.json");

            logger.info("Loading ExpressGateway Configuration file: {}", configurationFilesPath.toAbsolutePath());
            JsonObject globalData = JsonParser.parseString(Files.readString(configurationFilesPath)).getAsJsonObject();
            enforceConfigurationFileData = globalData.get("enforceConfigurationFileData").getAsBoolean();

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
        } catch (Exception e) {
            logger.error(e);
            System.exit(1);
        }
    }

    /**
     * Initialize global variables
     */
    private static void initGlobalVariables() {
        configurationDirectory = getPropertyOrEnv(CONFIGURATION_DIRECTORY.name(), "/etc/expressgateway/conf.d/default/");
        runningMode = RunningMode.valueOf(getPropertyOrEnv(RUNNING_MODE.name(), "STANDALONE").toUpperCase());
        clusterId = getPropertyOrEnv(CLUSTER_ID.name());

        restApiIpAddress = getPropertyOrEnv(REST_API_IP_ADDRESS.name());
        restApiPort = getPropertyOrEnvInt(REST_API_PORT.name());

        zooKeeperConnectionString = getPropertyOrEnv(ZOOKEEPER_CONNECTION_STRING.name());

        cryptoRestApiPkcs12File = getPropertyOrEnv(CRYPTO_REST_API_PKCS12_FILE.name());
        cryptoRestApiPassword = getPropertyOrEnv(CRYPTO_REST_API_PASSWORD.name());

        cryptoZooKeeperPkcs12File = getPropertyOrEnv(CRYPTO_ZOOKEEPER_PKCS12_FILE.name());
        cryptoZooKeeperPassword = getPropertyOrEnv(CRYPTO_ZOOKEEPER_PASSWORD.name());

        cryptoLoadBalancerPkcs12File = getPropertyOrEnv(CRYPTO_LOADBALANCER_PKCS12_FILE.name());
        cryptoLoadBalancerPkcs12File = getPropertyOrEnv(CRYPTO_LOADBALANCER_PASSWORD.name());
    }

    private static void initRestApiServer(JsonObject globalData) {
        JsonObject restApiData = globalData.getAsJsonObject("rest-api");

        // If Rest-Api data is null then we will load variables by environment variables.
        if (restApiData == null) {
            // If enforcing is set to 'true' then we will throw an error and shutdown.
            validateEnforce(enforceConfigurationFileData, "Rest-Api");

            // If Rest-Api IP Address environment variable is 'null' or empty then we will throw error and shutdown.
            checkNullOrEmptyEnv(restApiIpAddress, "Rest-Api IP Address");

            // If Rest-Api Port environment variable is 'null' or empty then we will throw error and shutdown.
            checkNullEnv(restApiPort, "Rest-Api Port");
        } else {
            // If Rest-Api IP Address data is 'null' or empty then we will throw error and shutdown.
            restApiIpAddress = checkNullOrEmptyConf(restApiData.get("ipAddress"), "Rest-Api IP Address");

            // If Rest-Api Port data is 'null' or empty then we will throw error and shutdown.
            restApiPort = Integer.parseInt(checkNullOrEmptyConf(restApiData.get("port"), "Rest-Api Port"));
        }
    }

    private static void initZooKeeper(JsonObject globalData) {
        JsonObject zookeeperData = globalData.getAsJsonObject("zookeeper");

        // If ZooKeeper data is null then we will load variables by environment variables.
        if (zookeeperData == null) {
            // If enforcing is set to 'true' then we will throw an error and shutdown.
            validateEnforce(enforceConfigurationFileData, "ZooKeeper");

            // If ZooKeeper ConnectionString environment variable is 'null' or empty then we will throw error and shutdown.
            checkNullOrEmptyEnv(zooKeeperConnectionString, "ZooKeeper ConnectionString");
        } else {
            // If ZooKeeper ConnectionString data is 'null' or empty then we will throw error and shutdown.
            zooKeeperConnectionString = checkNullOrEmptyConf(zookeeperData.get("connectionString"), "ZooKeeper ConnectionString");
        }
    }

    private static void initCrypto(JsonObject globalData) {
        JsonObject cryptoData = globalData.getAsJsonObject("crypto");
        if (cryptoData == null) {
            // If enforcing is set to 'true' then we will throw an error and shutdown.
            validateEnforce(enforceConfigurationFileData, "Crypto");

            // If Crypto Rest-Api Pkcs12 file environment variable is 'null' or empty then we will throw error and shutdown.
            checkNullOrEmptyEnv(cryptoRestApiPkcs12File, "Crypto Rest-Api PKCS12 File");

            // If Crypto Rest-Api Password environment variable is 'null' or empty then we will throw error and shutdown.
            checkNullOrEmptyEnv(cryptoRestApiPassword, "Crypto Rest-Api Password");
        } else {
            JsonElement restApiElement = cryptoData.get("rest-api");
            checkNullOrEmptyConf(restApiElement, "Crypto Rest-Api");
            checkNullOrEmptyConf(restApiElement.getAsJsonObject().get("pkcs12File"), "Crypto Rest-Api PKCS12 File");
            checkNullOrEmptyConf(restApiElement.getAsJsonObject().get("password"), "Crypto Rest-Api Password");

            // If Crypto Rest-Api PKCS12 is empty then the feature is disabled
            if (restApiElement.getAsJsonObject().get("pkcs12File").getAsString().isEmpty()) {
                logger.info("Crypto Rest-Api is disabled");
            } else {
                cryptoRestApiPkcs12File = restApiElement.getAsJsonObject().get("pkcs12File").getAsString();
                cryptoRestApiPassword = restApiElement.getAsJsonObject().get("password").getAsString();
            }

            // -----------------------------------------------------

            JsonElement zooKeeperElement = cryptoData.get("zookeeper");
            checkNullOrEmptyConf(zooKeeperElement, "Crypto ZooKeeper");
            checkNullOrEmptyConf(zooKeeperElement.getAsJsonObject().get("pkcs12File"), "Crypto ZooKeeper PKCS12 File");
            checkNullOrEmptyConf(zooKeeperElement.getAsJsonObject().get("password"), "Crypto ZooKeeper Password");

            // If Crypto ZooKeeper PKCS12 is empty then the feature is disabled
            if (zooKeeperElement.getAsJsonObject().get("pkcs12File").getAsString().isEmpty()) {
                logger.info("Crypto ZooKeeper is disabled");
            } else {
                cryptoZooKeeperPkcs12File = zooKeeperElement.getAsJsonObject().get("pkcs12File").getAsString();
                cryptoZooKeeperPassword = zooKeeperElement.getAsJsonObject().get("password").getAsString();
            }

            // -----------------------------------------------------

            JsonElement loadBalancerElement = cryptoData.get("loadBalancer");
            checkNullOrEmptyConf(loadBalancerElement, "Crypto LoadBalancer");
            checkNullOrEmptyConf(loadBalancerElement.getAsJsonObject().get("pkcs12File"), "Crypto Rest-Api PKCS12 File");
            checkNullOrEmptyConf(loadBalancerElement.getAsJsonObject().get("password"), "Crypto Rest-Api Password");

            // If Crypto LoadBalancer PKCS12 is empty then the feature is disabled
            if (loadBalancerElement.getAsJsonObject().get("pkcs12File").getAsString().isEmpty()) {
                logger.info("Crypto LoadBalancer is disabled");
            } else {
                cryptoLoadBalancerPkcs12File = loadBalancerElement.getAsJsonObject().get("pkcs12File").getAsString();
                cryptoLoadBalancerPassword = loadBalancerElement.getAsJsonObject().get("password").getAsString();
            }
        }
    }

    private static void initGlobalProperties() {
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

    private static void initRestApiServer() throws IOException, UnrecoverableKeyException, CertificateException, KeyStoreException, NoSuchAlgorithmException {
        if (cryptoRestApiPkcs12File.isEmpty()) {
            logger.info("Initializing Rest-Api Server without TLS");
        } else {
            logger.info("Loading Crypto Rest-Api PKCS12 File for TLS support");

            byte[] cryptoRestApiFile = Files.readAllBytes(Path.of(cryptoRestApiPkcs12File));
            try (ByteArrayInputStream inputStream = new ByteArrayInputStream(cryptoRestApiFile)) {
                CryptoEntry cryptoEntry = CryptoStore.fetchPrivateKeyCertificateEntry(inputStream, cryptoRestApiPassword.toCharArray(), "rest-api");
                RestApi.start(cryptoEntry);
            }

            logger.info("Successfully initialized Rest-Api Server with TLS");
        }
    }
}
