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
package com.shieldblaze.expressgateway.restapi.loadbalancer;

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

class L4HandlerTest {

    static ConfigurableApplicationContext ctx;
    static HttpClient httpClient = HttpClient.newHttpClient();

    @BeforeAll
    static void setup() {
        ctx = SpringApplication.run(Server.class);
    }

    @AfterAll
    static void teardown() {
        ctx.close();
    }

    @Test
    void createLoadBalancer() throws IOException, InterruptedException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("bindAddress", "127.0.0.1");
        requestBody.addProperty("bindPort", 20000);
        requestBody.addProperty("protocol", "udp");
        requestBody.addProperty("algorithm", "RoundRobin");
        requestBody.addProperty("sessionPersistence", "NOOP");

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .uri(URI.create("http://127.0.0.1:9110/loadbalancer/l4/create"))
                .setHeader("Content-Type", "application/json")
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(201, httpResponse.statusCode());

        JsonObject jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        assertTrue(jsonObject.get("Success").getAsBoolean());
        assertNotNull(jsonObject.get("Result").getAsJsonObject().get("LoadBalancerID").getAsString());
    }

    @Test
    void badBindAddress() throws IOException, InterruptedException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("bindAddress", "127.0.0.300");
        requestBody.addProperty("bindPort", 20000);
        requestBody.addProperty("protocol", "udp");
        requestBody.addProperty("algorithm", "RoundRobin");
        requestBody.addProperty("sessionPersistence", "NOOP");

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .uri(URI.create("http://127.0.0.1:9110/loadbalancer/l4/create"))
                .setHeader("Content-Type", "application/json")
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(400, httpResponse.statusCode());

        JsonObject jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        assertFalse(jsonObject.get("Success").getAsBoolean());
    }

    @Test
    void badBindPort() throws IOException, InterruptedException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("bindAddress", "127.0.0.1");
        requestBody.addProperty("bindPort", 99999);
        requestBody.addProperty("protocol", "udp");
        requestBody.addProperty("algorithm", "RoundRobin");
        requestBody.addProperty("sessionPersistence", "NOOP");

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .uri(URI.create("http://127.0.0.1:9110/loadbalancer/l4/create"))
                .setHeader("Content-Type", "application/json")
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(400, httpResponse.statusCode());

        JsonObject jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        assertFalse(jsonObject.get("Success").getAsBoolean());
    }


    @Test
    void badProtocol() throws IOException, InterruptedException {
        JsonObject requestBody = new JsonObject();
        requestBody.addProperty("bindAddress", "127.0.0.1");
        requestBody.addProperty("bindPort", 99999);
        requestBody.addProperty("protocol", "http");
        requestBody.addProperty("algorithm", "RoundRobin");
        requestBody.addProperty("sessionPersistence", "NOOP");

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .uri(URI.create("http://127.0.0.1:9110/loadbalancer/l4/create"))
                .setHeader("Content-Type", "application/json")
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(400, httpResponse.statusCode());

        JsonObject jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        assertFalse(jsonObject.get("Success").getAsBoolean());
    }
}
