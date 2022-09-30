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

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.common.MongoDB;
import com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil;
import com.shieldblaze.expressgateway.restapi.RestApi;
import dev.morphia.query.experimental.filters.Filters;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.UUID;

import static com.shieldblaze.expressgateway.bootstrap.Utils.checkNullEnv;
import static com.shieldblaze.expressgateway.bootstrap.Utils.checkNullOrEmptyConf;
import static com.shieldblaze.expressgateway.bootstrap.Utils.checkNullOrEmptyEnv;
import static com.shieldblaze.expressgateway.bootstrap.Utils.validateEnforce;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CLUSTER_ID;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_PASSWORD;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.MONGODB_CONNECTION_STRING;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.REST_API_IP_ADDRESS;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.REST_API_PORT;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.SYSTEM_ID;

/**
 * This class initializes and boots up the ExpressGateway.
 */
public final class Bootstrap {
    private static final Logger logger = LogManager.getLogger(Bootstrap.class);

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
            String applicationConfigurationDirectory = SystemPropertyUtil.getPropertyOrEnv("application.configuration.directory", "/etc/expressgateway/");
            String systemId;
            String restApiIpAddress = SystemPropertyUtil.getPropertyOrEnv(REST_API_IP_ADDRESS);
            Integer restApiPort = SystemPropertyUtil.getPropertyOrEnvInt(REST_API_PORT);
            String mongoDbConnectionString = SystemPropertyUtil.getPropertyOrEnv(MONGODB_CONNECTION_STRING);
            String clusterId = SystemPropertyUtil.getPropertyOrEnv(CLUSTER_ID);
            String cryptoPassword = SystemPropertyUtil.getPropertyOrEnv(CRYPTO_PASSWORD);

            Path configurationFilePath = Path.of(applicationConfigurationDirectory + "application.json");
            boolean applicationConfigurationFileExists = Files.exists(configurationFilePath, LinkOption.NOFOLLOW_LINKS);
            boolean enforceConfigurationFileData = false;

            JsonObject globalData = new JsonObject();
            if (applicationConfigurationFileExists) {
                System.out.println("Found Application Configuration file: " + configurationFilePath.toAbsolutePath());

                globalData = JsonParser.parseString(Files.readString(configurationFilePath)).getAsJsonObject();
                enforceConfigurationFileData = globalData.get("enforceConfigurationFileData").getAsBoolean();
            }

            Path systemIdFilePath = Path.of(applicationConfigurationDirectory + "system.id");
            boolean systemIdFileExists = Files.exists(systemIdFilePath, LinkOption.NOFOLLOW_LINKS);

            // If System ID file exists then read it else create a new System ID file
            if (systemIdFileExists) {
                systemId = Files.readString(systemIdFilePath);

                try {
                    systemId = UUID.fromString(systemId).toString();
                } catch (IllegalArgumentException ex) {
                    logger.fatal("Invalid System Id", ex);
                    logger.fatal("Shutting down...");
                    System.exit(1);
                    return;
                }
            } else {
                systemId = UUID.randomUUID().toString();
                Files.writeString(systemIdFilePath, systemId, StandardOpenOption.CREATE, StandardOpenOption.WRITE);
            }

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

            JsonObject databaseData = globalData.getAsJsonObject("database");
            if (databaseData == null) {
                // If enforcing is set to 'true' then we will throw an error and shutdown.
                validateEnforce(enforceConfigurationFileData, "Database");

                // If Database MongoDbConnectionString environment variable is 'null' or empty then we will throw error and shutdown.
                checkNullOrEmptyEnv(mongoDbConnectionString, "Database MongoDbConnectionString");

                // If Database ClusterId environment variable is 'null' or empty then we will throw error and shutdown.
                checkNullOrEmptyEnv(clusterId, "Database ClusterId");
            } else {
                // If Database MongoDbConnectionString is 'null' or empty then we will throw error and shutdown.
                mongoDbConnectionString = checkNullOrEmptyConf(databaseData.get("mongoDbConnectionString"), "Database MongoDbConnectionString");

                // If Database ClusterId is 'null' or empty then we will throw error and shutdown.
                clusterId = checkNullOrEmptyConf(databaseData.get("clusterId"), "Database ClusterId");
            }

            JsonObject cryptoData = globalData.getAsJsonObject("crypto");
            if (cryptoData == null) {
                // If enforcing is set to 'true' then we will throw an error and shutdown.
                validateEnforce(enforceConfigurationFileData, "Crypto");

                // If Crypto Password environment variable is 'null' or empty then we will throw error and shutdown.
                checkNullOrEmptyEnv(cryptoPassword, "Crypto Password");
            } else {
                // If Crypto Password is 'null' or empty then we will throw error and shutdown.
                cryptoPassword = checkNullOrEmptyConf(cryptoData.get("password"), "Crypto Password");
            }

            System.setProperty(SYSTEM_ID, systemId);
            System.setProperty(REST_API_IP_ADDRESS, restApiIpAddress);
            System.setProperty(REST_API_PORT, String.valueOf(restApiPort));
            System.setProperty(MONGODB_CONNECTION_STRING, mongoDbConnectionString);
            System.setProperty(CLUSTER_ID, clusterId);
            System.setProperty(CRYPTO_PASSWORD, cryptoPassword);

            // Wait for MongoDB connection to establish
            waitForMongoDbConnection();

            // Fetch Data from MongoDB
            fetchMetadataFromMongoDb(clusterId);

            RestApi.start();
        } catch (IOException e) {
            logger.error(e);
            System.exit(1);
        }
    }

    private static void waitForMongoDbConnection() {
        MongoDB.connectionFuture().whenComplete((aBoolean, throwable) -> {
            if (throwable == null) {
                logger.info("MongoDB connection successfully established");
            } else {
                logger.fatal("MongoDB connection could not be established", throwable);
                logger.fatal("Shutting down...");
                System.exit(1);
            }
        });
    }

    private static void fetchMetadataFromMongoDb(String clusterId) {
        ExpressGateway expressGateway = MongoDB.getInstance().find(ExpressGateway.class)
                .filter(Filters.eq("_id", clusterId))
                .first();
    }
}
