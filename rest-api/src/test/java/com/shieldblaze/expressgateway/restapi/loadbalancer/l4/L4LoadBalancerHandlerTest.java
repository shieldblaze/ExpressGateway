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
package com.shieldblaze.expressgateway.restapi.loadbalancer.l4;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.restapi.Server;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.springframework.boot.SpringApplication;
import org.springframework.context.ConfigurableApplicationContext;

import java.io.IOException;
import java.io.InputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class L4LoadBalancerHandlerTest {

    static ConfigurableApplicationContext ctx;
    static HttpClient httpClient = HttpClient.newHttpClient();

    @BeforeAll
    static void setup() {
        System.setProperty("restapi.bindPort", "9111");
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
        requestBody.addProperty("bindPort", 55555);
        requestBody.addProperty("protocol", "tcp");
        requestBody.addProperty("algorithm", "RoundRobin");
        requestBody.addProperty("sessionPersistence", "NOOP");

        HttpRequest httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .uri(URI.create("http://127.0.0.1:9111/loadbalancer/l4/create"))
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
        requestBody.addProperty("port", 50000);
        requestBody.addProperty("maxConnections", "-1");

        new SingleRequestTCPServer().start();

        httpRequest = HttpRequest.newBuilder()
                .POST(HttpRequest.BodyPublishers.ofString(requestBody.toString()))
                .setHeader("Content-Type", "application/json")
                .uri(URI.create("http://127.0.0.1:9111/node/" + LBID + "/add"))
                .build();

        httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        assertTrue(jsonObject.get("Success").getAsBoolean());

        Socket socket = new Socket(InetAddress.getByName("127.0.0.1"), 55555);
        InputStream inputStream = socket.getInputStream();
        assertEquals("HELLO", new String(inputStream.readNBytes(5)));
        socket.close();
    }

    private static final class SingleRequestTCPServer extends Thread {
        @Override
        public void run() {
            try (ServerSocket serverSocket = new ServerSocket(50000, 1000, InetAddress.getByName("127.0.0.1"))) {
               Socket socket = serverSocket.accept();
               socket.getOutputStream().write("HELLO".getBytes());
               socket.getOutputStream().flush();
               socket.close();
            } catch (Exception ex) {
                ex.printStackTrace();
            }
        }
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
                .uri(URI.create("http://127.0.0.1:9111/loadbalancer/l4/create"))
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
                .uri(URI.create("http://127.0.0.1:9111/loadbalancer/l4/create"))
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
                .uri(URI.create("http://127.0.0.1:9111/loadbalancer/l4/create"))
                .setHeader("Content-Type", "application/json")
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(400, httpResponse.statusCode());

        JsonObject jsonObject = JsonParser.parseString(httpResponse.body()).getAsJsonObject();
        assertFalse(jsonObject.get("Success").getAsBoolean());
    }
}
