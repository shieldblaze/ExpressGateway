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
package com.shieldblaze.expressgateway.restapi.loadbalancer.http;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.restapi.Server;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.springframework.boot.SpringApplication;
import org.springframework.context.ConfigurableApplicationContext;

import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

import static org.junit.jupiter.api.Assertions.*;

class HTTPLoadBalancerHandlerTest {

    static ConfigurableApplicationContext ctx;
    static HttpClient httpClient = HttpClient.newHttpClient();

    @BeforeAll
    static void setup() {
        System.setProperty("restapi.bindPort", "9112");
        ctx = SpringApplication.run(Server.class);
    }

    @AfterAll
    static void teardown() {
        ctx.close();
    }

    @Test
    void testCreation() throws IOException, InterruptedException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("bindAddress", "127.0.0.1");
        requestBody.addProperty("bindPort", 10000);
        requestBody.addProperty("algorithm", "HTTPRoundRobin");
        requestBody.addProperty("sessionPersistence", "NOOP");

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .uri(URI.create("http://127.0.0.1:9112/loadbalancer/http/create"))
                .setHeader("Content-Type", "application/json")
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(201, httpResponse.statusCode());

        JsonObject jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        String LBID = jsonObject.get("Result").getAsJsonObject().get("LoadBalancerID").getAsString();

        assertTrue(jsonObject.get("Success").getAsBoolean());
        assertNotNull(LBID);

        // Add a node
        requestBody = new JsonObject();
        requestBody.addProperty("host", "127.0.0.1");
        requestBody.addProperty("port", 25000);
        requestBody.addProperty("maxConnections", "-1");

        httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .setHeader("Content-Type", "application/json")
                .uri(URI.create("http://127.0.0.1:9112/node/" + LBID + "/add"))
                .build();

        httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        assertTrue(jsonObject.get("Success").getAsBoolean());

        String NodeID = jsonObject.get("Result").getAsJsonObject().get("NodeID").getAsString();

        httpRequest = HttpRequest.newBuilder()
                .DELETE()
                .setHeader("Content-Type", "application/json")
                .uri(URI.create("http://127.0.0.1:9112/node/" + LBID + "/remove/" + NodeID))
                .build();

        httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();

        assertTrue(jsonObject.get("Success").getAsBoolean());
    }
}
