/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.restapi.config;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;
import org.springframework.boot.SpringApplication;
import org.springframework.context.ConfigurableApplicationContext;

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

import static org.junit.jupiter.api.Assertions.assertEquals;

@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class TransportHandlerTransformerTest {
    final static HttpClient HTTP_CLIENT = HttpClient.newHttpClient();
    static ConfigurableApplicationContext ctx;

    @BeforeAll
    static void setup() {
        System.setProperty("egw.config.dir", System.getProperty("java.io.tmpdir"));
        ctx = SpringApplication.run(Server.class);
    }

    @AfterAll
    static void teardown() {
        ctx.close();
    }

    @Test
    @Order(1)
    void create() throws IOException, InterruptedException {
        JsonObject configJson = new JsonObject();
        configJson.addProperty("transportType", "NIO");
        configJson.addProperty("receiveBufferAllocationType", "FIXED");
        JsonArray jsonArray = new JsonArray();
        jsonArray.add(9001);
        configJson.add("receiveBufferSizes", jsonArray);
        configJson.addProperty("tcpConnectionBacklog", 100000);
        configJson.addProperty("socketReceiveBufferSize", 2147483647);
        configJson.addProperty("socketSendBufferSize", 2147483647);
        configJson.addProperty("tcpFastOpenMaximumPendingRequests", 100000);
        configJson.addProperty("backendConnectTimeout", 2147483647);
        configJson.addProperty("connectionIdleTimeout", 2147483647);

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(configJson.toString()))
                .uri(URI.create("http://127.0.0.1:9110/config/transport"))
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(204, httpResponse.statusCode());
    }

    @Test
    @Order(2)
    void get() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://127.0.0.1:9110/config/transport"))
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());

        JsonObject jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        assertEquals("NIO", jsonObject.get("transportType").getAsString());
        assertEquals("FIXED", jsonObject.get("receiveBufferAllocationType").getAsString());
        assertEquals(9001, jsonObject.get("receiveBufferSizes").getAsJsonArray().get(0).getAsInt());
        assertEquals(100000, jsonObject.get("tcpConnectionBacklog").getAsInt());
        assertEquals(2147483647, jsonObject.get("socketReceiveBufferSize").getAsInt());
        assertEquals(2147483647, jsonObject.get("socketSendBufferSize").getAsInt());
        assertEquals(100000, jsonObject.get("tcpFastOpenMaximumPendingRequests").getAsInt());
        assertEquals(2147483647, jsonObject.get("backendConnectTimeout").getAsInt());
        assertEquals(2147483647, jsonObject.get("connectionIdleTimeout").getAsInt());
    }

    @Test
    @Order(3)
    void delete() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .DELETE()
                .uri(URI.create("http://127.0.0.1:9110/config/transport"))
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(204, httpResponse.statusCode());
    }

    @Test
    @Order(4)
    void testDelete() throws IOException, InterruptedException {
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://127.0.0.1:9110/config/transport"))
                .build();

        HttpResponse<String> httpResponse = HTTP_CLIENT.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(404, httpResponse.statusCode());
    }
}
