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
import com.shieldblaze.expressgateway.restapi.RestApi;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.FileNotFoundException;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Scanner;

/**
 * This class initializes and boots up the ExpressGateway.
 */
public final class ExpressGateway {
    private static final Logger logger = LogManager.getLogger(ExpressGateway.class);

    static {
        System.setProperty("StartedAt", String.valueOf(System.currentTimeMillis()));
    }

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
            JsonObject data = JsonParser.parseString(Files.readString(
                    Path.of("/etc/expressgateway/application.json"))).getAsJsonObject();

            String state = data.get("state").getAsString();
            String profile = data.get("profile").getAsString();

            JsonObject restApi = data.get("restApi").getAsJsonObject();
            String restApiIpAddress = restApi.get("ipAddress").getAsString();
            int restApiPort = restApi.get("port").getAsInt();

            String alias;
            String password;
            if (restApi.get("dataStorePassword").isJsonNull()) {
                try (Scanner scanner = new Scanner(System.in)) {
                    System.out.print("Enter DataStore Alias: ");
                    alias = scanner.nextLine();

                    System.out.print("Enter DataStore Password: ");
                    password = scanner.nextLine();
                }
            } else {
                alias = restApi.get("dataStoreAlias").getAsString();
                password = restApi.get("dataStorePassword").getAsString();
            }

            // DataStore
            System.setProperty("datastore.alias", alias);
            System.setProperty("datastore.password", password);

            // Rest-Api
            System.setProperty("restapi.address", restApiIpAddress);
            System.setProperty("restapi.port", String.valueOf(restApiPort));

            RestApi.start();
        } catch (FileNotFoundException e) {
            logger.error("File not found in '/etc/expressgateway/application.json'");
            System.exit(1);
        } catch (IOException e) {
            logger.error(e);
            System.exit(1);
        }
    }
}
