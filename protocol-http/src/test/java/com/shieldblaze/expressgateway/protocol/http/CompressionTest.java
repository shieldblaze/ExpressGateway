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
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.Cipher;
import com.shieldblaze.expressgateway.configuration.tls.MutualTLS;
import com.shieldblaze.expressgateway.configuration.tls.Protocol;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.tls.TLSServerMapping;
import com.shieldblaze.expressgateway.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
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

    static TransportConfiguration transportConfiguration;
    static EventLoopConfiguration eventLoopConfiguration;
    static CoreConfiguration coreConfiguration;
    static TLSConfiguration forServer;
    static TLSConfiguration forClient;
    static HTTPConfiguration httpConfiguration;
    static HttpClient httpClient;

    @BeforeAll
    static void initialize() throws Exception {
        transportConfiguration = TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withTCPFastOpenMaximumPendingRequests(2147483647)
                .withBackendConnectTimeout(10000 * 5)
                .withBackendSocketTimeout(10000 * 5)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(new int[]{65535})
                .withSocketReceiveBufferSize(2147483647)
                .withSocketSendBufferSize(2147483647)
                .withTCPConnectionBacklog(2147483647)
                .withDataBacklog(2147483647)
                .withConnectionIdleTimeout(1800000)
                .build();

        eventLoopConfiguration = EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(2)
                .withChildWorkers(4)
                .build();

        coreConfiguration = CoreConfigurationBuilder.newBuilder()
                .withTransportConfiguration(transportConfiguration)
                .withEventLoopConfiguration(eventLoopConfiguration)
                .withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration.DEFAULT)
                .build();

        SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);

        CertificateKeyPair certificateKeyPair = new CertificateKeyPair(
                Collections.singletonList(selfSignedCertificate.cert()), selfSignedCertificate.key(), false);

        TLSServerMapping tlsServerMapping = new TLSServerMapping(certificateKeyPair);

        forServer = TLSConfigurationBuilder.forServer()
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_128_GCM_SHA256))
                .withUseALPN(true)
                .withTLSServerMapping(tlsServerMapping)
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .build();

        forClient = TLSConfigurationBuilder.forClient()
                .withProtocols(Collections.singletonList(Protocol.TLS_1_3))
                .withCiphers(Collections.singletonList(Cipher.TLS_AES_256_GCM_SHA384))
                .withUseALPN(true)
                .withMutualTLS(MutualTLS.NOT_REQUIRED)
                .withTrustManager(InsecureTrustManagerFactory.INSTANCE.getTrustManagers()[0])
                .build();

        httpConfiguration = HTTPConfigurationBuilder.newBuilder()
                .withBrotliCompressionLevel(4)
                .withCompressionThreshold(100)
                .withDeflateCompressionLevel(6)
                .withMaxChunkSize(1024 * 100)
                .withMaxContentLength(1024 * 10240)
                .withMaxHeaderSize(1024 * 10)
                .withMaxInitialLineLength(1024 * 100)
                .withH2InitialWindowSize(Integer.MAX_VALUE)
                .withH2MaxConcurrentStreams(1000)
                .withH2MaxHeaderSizeList(262144)
                .withH2MaxFrameSize(16777215)
                .withH2MaxHeaderTableSize(65536)
                .build();

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

        Cluster cluster = new ClusterPool(new EventStream(), new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));
        new Node(cluster, new InetSocketAddress("127.0.0.1", 10000));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(coreConfiguration)
                .withHTTPConfiguration(httpConfiguration)
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("127.0.0.1", 20000))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.success());

        // Brotli only
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://127.0.0.1:20000"))
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
                    .uri(URI.create("https://127.0.0.1:20000"))
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
                    .uri(URI.create("https://127.0.0.1:20000"))
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

        Cluster cluster = new ClusterPool(new EventStream(), new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));
        new Node(cluster, new InetSocketAddress("127.0.0.1", 10001));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(coreConfiguration)
                .withHTTPConfiguration(httpConfiguration)
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("127.0.0.1", 20001))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.success());

        // Gzip only
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://127.0.0.1:20001"))
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
                    .uri(URI.create("https://127.0.0.1:20001"))
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

        Cluster cluster = new ClusterPool(new EventStream(), new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE));
        new Node(cluster, new InetSocketAddress("127.0.0.1", 10002));

        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withCoreConfiguration(coreConfiguration)
                .withHTTPConfiguration(httpConfiguration)
                .withTLSForClient(forClient)
                .withTLSForServer(forServer)
                .withCluster(cluster)
                .withBindAddress(new InetSocketAddress("127.0.0.1", 20002))
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .withL4FrontListener(new TCPListener())
                .build();

        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = httpLoadBalancer.start();
        l4FrontListenerStartupEvent.future().join();
        assertTrue(l4FrontListenerStartupEvent.success());

        // Deflate only
        {
            HttpRequest httpRequest = HttpRequest.newBuilder()
                    .GET()
                    .uri(URI.create("https://127.0.0.1:20002"))
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
