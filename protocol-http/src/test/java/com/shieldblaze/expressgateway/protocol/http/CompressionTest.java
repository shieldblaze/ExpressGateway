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

import com.aayushatharva.brotli4j.decoder.DecoderJNI;
import com.aayushatharva.brotli4j.decoder.DirectDecompress;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import io.netty.handler.ssl.util.SelfSignedCertificate;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLContext;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.security.SecureRandom;
import java.time.Duration;
import java.util.Collections;
import java.util.zip.DataFormatException;
import java.util.zip.GZIPInputStream;
import java.util.zip.Inflater;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class CompressionTest {

    static TLSConfiguration forServer;
    static TLSConfiguration forClient;
    static HttpClient httpClient;

    @BeforeAll
    static void initialize() throws Exception {
        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(Collections.singletonList(selfSignedCertificate.cert()), selfSignedCertificate.key());

        forServer = TLSConfiguration.DEFAULT_SERVER;
        forServer.addMapping("localhost", certificateKeyPair);

        forClient = TLSConfiguration.DEFAULT_CLIENT;
        forClient.acceptAllCerts(true);

        SSLContext sslContext = SSLContext.getInstance("TLSv1.3");
        sslContext.init(null, InsecureTrustManagerFactory.INSTANCE.getTrustManagers(), new SecureRandom());

        httpClient = HttpClient.newBuilder()
                .sslContext(sslContext)
                .build();
    }

    @Test
    void brotliCompressionTest() throws InterruptedException, IOException {

        @ChannelHandler.Sharable
        class Handler extends SimpleChannelInboundHandler<FullHttpRequest> {
            @Override
            protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
                DefaultFullHttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
                        HttpResponseStatus.OK, Unpooled.wrappedBuffer("Meow".getBytes()));
                if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                    httpResponse.headers().set("x-http2-stream-id", msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
                } else {
                    httpResponse.headers().set(HttpHeaderNames.CONTENT_LENGTH, 4);
                }
                httpResponse.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/html");
                ctx.writeAndFlush(httpResponse);
            }
        }

        HTTPServer httpServer = new HTTPServer(10000, true, new Handler());
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = new ClusterPool(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(CoreConfiguration.DEFAULT)
                .withHTTPConfiguration(HTTPConfiguration.DEFAULT)
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withBindAddress(new InetSocketAddress("localhost", 20000))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .withEventStream(new EventStream())
                .build();

        httpLoadBalancer.mapCluster("localhost:20000", cluster);
        new Node(cluster, new InetSocketAddress("localhost", 10000));

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.success());

        // Brotli only
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://localhost:20000"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "br")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("br", httpResponse.headers().firstValue("Content-Encoding").get());

            DirectDecompress directDecompress = DirectDecompress.decompress(httpResponse.body());
            assertEquals(DecoderJNI.Status.DONE, directDecompress.getResultStatus());
            assertEquals("Meow", new String(directDecompress.getDecompressedData()));
        }

        // Brotli and Gzip
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://localhost:20000"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "gzip, br")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("br", httpResponse.headers().firstValue("Content-Encoding").get());

            DirectDecompress directDecompress = DirectDecompress.decompress(httpResponse.body());
            assertEquals(DecoderJNI.Status.DONE, directDecompress.getResultStatus());
            assertEquals("Meow", new String(directDecompress.getDecompressedData()));
        }

        // Brotli, Gzip and Deflate
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://localhost:20000"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "gzip, deflate, br")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("br", httpResponse.headers().firstValue("Content-Encoding").get());

            DirectDecompress directDecompress = DirectDecompress.decompress(httpResponse.body());
            assertEquals(DecoderJNI.Status.DONE, directDecompress.getResultStatus());
            assertEquals("Meow", new String(directDecompress.getDecompressedData()));
        }

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.success());
    }

    @Test
    void gzipCompressionTest() throws InterruptedException, IOException {
        @ChannelHandler.Sharable
        class Handler extends SimpleChannelInboundHandler<FullHttpRequest> {
            @Override
            protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
                DefaultFullHttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
                        HttpResponseStatus.OK, Unpooled.wrappedBuffer("Meow".getBytes()));
                if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                    httpResponse.headers().set("x-http2-stream-id", msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
                } else {
                    httpResponse.headers().set(HttpHeaderNames.CONTENT_LENGTH, 4);
                }
                httpResponse.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/html");
                ctx.writeAndFlush(httpResponse);
            }
        }

        HTTPServer httpServer = new HTTPServer(10001, true, new Handler());
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = new ClusterPool(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(CoreConfiguration.DEFAULT)
                .withHTTPConfiguration(HTTPConfiguration.DEFAULT)
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withBindAddress(new InetSocketAddress("localhost", 20001))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .withEventStream(new EventStream())
                .build();

        httpLoadBalancer.mapCluster("localhost:20001", cluster);
        new Node(cluster, new InetSocketAddress("localhost", 10001));

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.success());

        // Gzip only
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://localhost:20001"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "gzip")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("gzip", httpResponse.headers().firstValue("Content-Encoding").get());

            GZIPInputStream gzipInputStream = new GZIPInputStream(new ByteArrayInputStream(httpResponse.body()));
            assertEquals("Meow", new String(gzipInputStream.readAllBytes()));
            gzipInputStream.close();
        }

        // Gzip and Deflate
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://localhost:20001"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "gzip, deflate")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("gzip", httpResponse.headers().firstValue("Content-Encoding").get());

            GZIPInputStream gzipInputStream = new GZIPInputStream(new ByteArrayInputStream(httpResponse.body()));
            assertEquals("Meow", new String(gzipInputStream.readAllBytes()));
            gzipInputStream.close();
        }

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.success());
    }

    @Test
    void deflateCompressionTest() throws InterruptedException, IOException, DataFormatException {
        @ChannelHandler.Sharable
        class Handler extends SimpleChannelInboundHandler<FullHttpRequest> {
            @Override
            protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
                DefaultFullHttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1,
                        HttpResponseStatus.OK, Unpooled.wrappedBuffer("Meow".getBytes()));
                if (msg.headers().contains(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())) {
                    httpResponse.headers().set("x-http2-stream-id", msg.headers().get(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text()));
                } else {
                    httpResponse.headers().set(HttpHeaderNames.CONTENT_LENGTH, 4);
                }
                httpResponse.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/html");
                ctx.writeAndFlush(httpResponse);
            }
        }

        HTTPServer httpServer = new HTTPServer(10002, true, new Handler());
        httpServer.start();
        Thread.sleep(500L);

        Cluster cluster = new ClusterPool(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(CoreConfiguration.DEFAULT)
                .withHTTPConfiguration(HTTPConfiguration.DEFAULT)
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withBindAddress(new InetSocketAddress("localhost", 20002))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .withEventStream(new EventStream())
                .build();

        httpLoadBalancer.mapCluster("localhost:20002", cluster);
        new Node(cluster, new InetSocketAddress("localhost", 10002));

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.success());

        // Deflate only
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://localhost:20002"))
                    .version(HttpClient.Version.HTTP_2)
                    .timeout(Duration.ofSeconds(5))
                    .setHeader("Accept-Encoding", "deflate")
                    .build();

            HttpResponse<byte[]> httpResponse = httpClient.send(httpRequest, HttpResponse.BodyHandlers.ofByteArray());
            assertEquals(200, httpResponse.statusCode());
            assertEquals("deflate", httpResponse.headers().firstValue("Content-Encoding").get());

            Inflater inflater = new Inflater();
            inflater.setInput(httpResponse.body());
            byte[] result = new byte[1024];
            int length = inflater.inflate(result);
            inflater.end();
            assertEquals("Meow", new String(result, 0, length));
        }

        // Shutdown HTTP Server and Load Balancer
        httpServer.shutdown();
        L4FrontListenerStopEvent l4FrontListenerStopEvent = httpLoadBalancer.stop();
        l4FrontListenerStopEvent.future().join();
        assertTrue(l4FrontListenerStopEvent.success());
    }
}
