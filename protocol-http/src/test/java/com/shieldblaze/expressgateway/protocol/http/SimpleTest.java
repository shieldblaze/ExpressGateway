/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfigurationBuilder;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLContext;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.security.SecureRandom;
import java.time.Duration;
import java.util.Collections;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class SimpleTest {

    static TLSConfiguration forServer;
    static TLSConfiguration forClient;
    static HttpClient httpClient;

    @BeforeAll
    static void initialize() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);
        CertificateKeyPair certificateKeyPair = CertificateKeyPair.forClient(Collections.singletonList(selfSignedCertificate.cert()), selfSignedCertificate.key());

        forServer = TLSConfiguration.DEFAULT_SERVER;
        forServer.addMapping("localhost", certificateKeyPair);

        forClient = TLSConfigurationBuilder.forClient()
                .withAcceptAllCertificate(true)
                .build();
        forClient.defaultMapping(CertificateKeyPair.defaultClientInstance());

        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

        httpClient = HttpClient.newBuilder()
                .sslContext(sslContext)
                .build();
    }

    @Test
    void http2BackendWithTLSClient() throws Exception {
        HTTPServer httpServer = new HTTPServer(10000, true);
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withBindAddress(new InetSocketAddress("localhost", 20000))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mapCluster("localhost:20000", cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("localhost", 10000))
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.isSuccess());

        // Send using HTTP/1.1
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:20000"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(5))
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Send using HTTP/2
        httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:20000"))
                .version(HttpClient.Version.HTTP_2)
                .timeout(Duration.ofSeconds(5))
                .build();

        httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccess());
    }

    @Test
    void http1BackendWithTLSClient() throws Exception {
        HTTPServer httpServer = new HTTPServer(10001, false);
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withTLSForServer(forServer)
                .withBindAddress(new InetSocketAddress("localhost", 20001))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mapCluster("localhost:20001", cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("localhost", 10001))
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.isSuccess());

        // Send using HTTP/1.1
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:20001"))
                .version(HttpClient.Version.HTTP_2)
                .timeout(Duration.ofSeconds(5))
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Send using HTTP/2
        httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("https://localhost:20001"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(5))
                .build();

        httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccess());
    }

    @Test
    void http2BackendWithoutTLSClient() throws Exception {
        HTTPServer httpServer = new HTTPServer(10002, true);
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withTLSForClient(forClient)
                .withBindAddress(new InetSocketAddress("localhost", 20002))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mapCluster("localhost:20002", cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("localhost", 10002))
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.isSuccess());

        // Send using HTTP/1.1
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://localhost:20002"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(5))
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccess());
    }

    @Test
    void http1BackendWithoutTLSClient() throws Exception {
        HTTPServer httpServer = new HTTPServer(55555, false);
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress("localhost", 20003))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mapCluster("localhost:20003", cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("localhost", 55555))
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.isSuccess());

        // Send using HTTP/1.1
        HttpRequest httpRequest = HttpRequest.newBuilder()
                .GET()
                .uri(URI.create("http://localhost:20003"))
                .version(HttpClient.Version.HTTP_1_1)
                .timeout(Duration.ofSeconds(5))
                .build();

        HttpResponse<String> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofString());
        assertEquals(200, httpResponse.statusCode());
        assertEquals("Meow", httpResponse.body());

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.isSuccess());
    }
}
