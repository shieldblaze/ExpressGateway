package com.shieldblaze.expressgateway.core.server.http;

import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpMessage;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.DefaultHttp2SettingsFrame;
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2Settings;
import io.netty.handler.codec.http2.Http2Stream;
import io.netty.handler.codec.http2.HttpConversionUtil;

final class HTTP2ClientTranslationHandler extends Http2ChannelDuplexHandler {

    private Http2FrameStream http2FrameStream;

    @Override
    public void channelActive(ChannelHandlerContext ctx) {
        Http2Settings http2Settings = new Http2Settings();
        http2Settings.pushEnabled(false);
        http2Settings.maxConcurrentStreams(100);
        http2Settings.initialWindowSize(1073741824);

        DefaultHttp2SettingsFrame defaultHttp2SettingsFrame = new DefaultHttp2SettingsFrame(http2Settings);
        ctx.writeAndFlush(defaultHttp2SettingsFrame);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        if (msg instanceof HttpRequest) {
            if (http2FrameStream == null) {
                http2FrameStream = newStream();
            }

            HttpRequest httpRequest = (HttpRequest) msg;
            httpRequest.headers().set(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(), "https");
            httpRequest.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), http2FrameStream.id());

            boolean endStream = true;
            if (httpRequest.headers().getInt(HttpHeaderNames.CONTENT_LENGTH, 0) > 0) {
                endStream = false;
            } else if (httpRequest.headers().contains(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED, true)) {
                endStream = false;
            }

            Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers((HttpMessage) msg, true);
            DefaultHttp2HeadersFrame defaultHttp2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, endStream);
            defaultHttp2HeadersFrame.stream(http2FrameStream);

            ctx.writeAndFlush(defaultHttp2HeadersFrame);
        } else if (msg instanceof HttpContent) {
            DefaultHttp2DataFrame defaultHttp2DataFrame;
            if (msg instanceof LastHttpContent) {
                defaultHttp2DataFrame = new DefaultHttp2DataFrame(true);
            } else {
                defaultHttp2DataFrame = new DefaultHttp2DataFrame(((HttpContent) msg).content());
            }
            defaultHttp2DataFrame.stream(http2FrameStream);
            ctx.writeAndFlush(defaultHttp2DataFrame);
        } else {
            System.err.println("[Write Client] Unknown Message: " + msg);
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame) {
            onHeadersRead(ctx, (Http2HeadersFrame) msg);
        } else if (msg instanceof Http2DataFrame) {
            onDataRead(ctx, (Http2DataFrame) msg);
        } else {
            System.err.println("[Read Client] Unknown Message: " + msg);
        }
    }

    private void onHeadersRead(ChannelHandlerContext ctx, Http2HeadersFrame headers) throws Exception {
        HttpResponse httpResponse = HttpConversionUtil.toHttpResponse(http2FrameStream.id(), headers.headers(), true);
        httpResponse.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());
        httpResponse.headers().remove(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text());

        if (headers.isEndStream()) {
            httpResponse.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
        }

        super.channelRead(ctx, httpResponse);
    }

    private void onDataRead(ChannelHandlerContext ctx, Http2DataFrame data) throws Exception {
        HttpContent httpContent;
        if (data.isEndStream()) {
            httpContent = new DefaultLastHttpContent(data.content());
        } else {
            httpContent = new DefaultHttpContent(data.content());
        }
        super.channelRead(ctx, httpContent);
    }

    static class DefaultHttp2FrameStream implements Http2FrameStream {

        private final int id;
        private Http2Stream.State state;

        DefaultHttp2FrameStream(int id) {
            this.id = id;
            this.state = Http2Stream.State.OPEN;
        }

        @Override
        public int id() {
            return id;
        }

        @Override
        public Http2Stream.State state() {
            return state;
        }

        void setState(Http2Stream.State state) {
            this.state = state;
        }

        @Override
        public String toString() {
            return "DefaultHttp2FrameStream{" +
                    "id=" + id +
                    ", state=" + state +
                    '}';
        }
    }
}
