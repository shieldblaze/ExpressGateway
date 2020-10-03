package com.shieldblaze.expressgateway.core.server.http;

import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpMessage;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;

final class HTTP2ServerTranslationHandler extends ChannelDuplexHandler {

    private Http2FrameStream stream;
    private int dependencyId;
    private short weight;
    private int streamId;

    public HTTP2ServerTranslationHandler() {
        System.out.println("Init Again");
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame) {
            onHeadersRead(ctx, (Http2HeadersFrame) msg);
        } else if (msg instanceof Http2DataFrame) {
            onDataRead(ctx, (Http2DataFrame) msg);
        } else {
            System.err.println("[Read Server] Unknown Message: " + msg);
        }
    }

    private void onHeadersRead(ChannelHandlerContext ctx, Http2HeadersFrame headers) throws Exception {
        if (stream == null) {
            stream = headers.stream();
        }

        HttpRequest httpRequest = HttpConversionUtil.toHttpRequest(headers.stream().id(), headers.headers(), true);

        if (!headers.isEndStream()) {
            httpRequest.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
        }

        this.dependencyId = httpRequest.headers().getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text(), -1);
        this.weight = httpRequest.headers().getShort(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text(), (short) -1);
        this.streamId = httpRequest.headers().getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());

        httpRequest.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text());
        httpRequest.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text());
        httpRequest.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());
        httpRequest.headers().remove(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text());

        super.channelRead(ctx, httpRequest);
    }

    private void onDataRead(ChannelHandlerContext ctx, Http2DataFrame data) throws Exception {
        HttpContent httpContent;
        if (data.isEndStream()) {
            httpContent = LastHttpContent.EMPTY_LAST_CONTENT;
        } else {
            httpContent = new DefaultHttpContent(data.content());
        }
        super.channelRead(ctx, httpContent);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        if (msg instanceof HttpResponse) {
            HttpResponse httpResponse = (HttpResponse) msg;
            if (dependencyId != -1) {
                httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text(), dependencyId);
            }
            if (weight != -1) {
                httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text(), weight);
            }
            httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), streamId);

            Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers((HttpMessage) msg, true);
            DefaultHttp2HeadersFrame defaultHttp2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers);
            defaultHttp2HeadersFrame.stream(stream);
            ctx.writeAndFlush(defaultHttp2HeadersFrame);
        } else if (msg instanceof HttpContent) {
            Http2DataFrame dataFrame;

            if (msg instanceof LastHttpContent) {
                dataFrame = new DefaultHttp2DataFrame(((LastHttpContent) msg).content(), true);
            } else {
                dataFrame = new DefaultHttp2DataFrame(((HttpContent) msg).content());
            }

            dataFrame.stream(stream);
            ctx.writeAndFlush(dataFrame);
        } else {
            System.err.println("[Write Server] Unknown Message: " + msg);
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) throws Exception {
        cause.printStackTrace();
    }
}
