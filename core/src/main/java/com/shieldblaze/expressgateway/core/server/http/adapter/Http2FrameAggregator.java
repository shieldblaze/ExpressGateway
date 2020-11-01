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
package com.shieldblaze.expressgateway.core.server.http.adapter;

import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2StreamFrame;

import java.util.Map;
import java.util.concurrent.ConcurrentSkipListMap;

final class Http2FrameAggregator extends ChannelInboundHandlerAdapter {

    private final Map<Http2FrameStream, Aggregation> streamMap = new ConcurrentSkipListMap<>(Http2FrameStreamComparator.INSTANCE);

    private final boolean aggregateDataFrame;

    Http2FrameAggregator() {
        this(false);
    }

    Http2FrameAggregator(boolean aggregateDataFrame) {
        this.aggregateDataFrame = aggregateDataFrame;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (msg instanceof Http2HeadersFrame) {
            Http2HeadersFrame headersFrame = (Http2HeadersFrame) msg;
            Aggregation aggregation = streamMap.get(headersFrame.stream());

            /*
             * If 'aggregation' is 'null' then this frame is new.
             *
             * If 'aggregation' is not 'null' then this frame is part of continuation frame.
             * In this case, we'll append the append this frame headers to old frame headers.
             */
            if (aggregation == null) {

                // If 'endOfStream' is 'true' then we don't need to aggregate anything.
                if (headersFrame.isEndStream()) {
                    ctx.fireChannelRead(headersFrame);
                } else {
                    aggregation = new Aggregation();
                    aggregation.state = Aggregation.State.DOING_HEADERS;
                    aggregation.http2StreamFrame = headersFrame;
                    streamMap.put(headersFrame.stream(), aggregation);
                }
            } else {
                Http2HeadersFrame http2HeadersFrame = (Http2HeadersFrame) aggregation.http2StreamFrame;
                http2HeadersFrame.headers().setAll(headersFrame.headers());
            }
        } else if (msg instanceof Http2DataFrame) {
            Http2DataFrame dataFrame = (Http2DataFrame) msg;
            Aggregation aggregation = streamMap.get(dataFrame.stream());

            if (aggregation.state == Aggregation.State.DOING_HEADERS) {
                ctx.fireChannelRead(aggregation.http2StreamFrame);
                aggregation.state = Aggregation.State.DONE_HEADERS;
                aggregation.http2StreamFrame = null;
            }

            if (aggregateDataFrame) {
                DefaultHttp2DataFrame defaultHttp2DataFrame = new DefaultHttp2DataFrame(ctx.alloc().buffer(), false, 0);
                defaultHttp2DataFrame.content().writeBytes(dataFrame.content());
                aggregation.state = Aggregation.State.DOING_DATA;
                aggregation.http2StreamFrame = defaultHttp2DataFrame;
            } else {
                aggregation.state = Aggregation.State.DOING_DATA;
                aggregation.http2StreamFrame = dataFrame;
            }

            if (dataFrame.isEndStream()) {
                ctx.fireChannelRead(aggregation.http2StreamFrame);
                aggregation.state = Aggregation.State.DONE_DATA;
                aggregation.http2StreamFrame = null;
            }

        } else {
            // Fire everything else down to pipeline.
            ctx.fireChannelRead(msg);
        }
    }

    private static final class Aggregation {

        private enum State {
            DOING_HEADERS,
            DONE_HEADERS,
            DOING_DATA,
            DONE_DATA,
            DOING_TRAILERS,
            DONE_TRAILERS
        }

        private State state;
        private Http2StreamFrame http2StreamFrame;
    }
}
