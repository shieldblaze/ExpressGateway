/*
 * Copyright 2020 The Netty Project
 *
 * The Netty Project licenses this file to you under the Apache License,
 * version 2.0 (the "License"); you may not use this file except in compliance
 * with the License. You may obtain a copy of the License at:
 *
 *   http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
 * WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the
 * License for the specific language governing permissions and limitations
 * under the License.
 */
package io.netty.handler.codec.http2;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.util.internal.StringUtil;

/**
 * {@link DefaultHttp2TranslatedHttpContent} contains {@code streamId}
 * from last (endOfStream) {@link Http2DataFrame}.
 */
public class DefaultHttp2TranslatedLastHttpContent extends DefaultLastHttpContent {

    private final int streamId;

    public DefaultHttp2TranslatedLastHttpContent(int streamId) {
        this(Unpooled.EMPTY_BUFFER, streamId);
    }

    public DefaultHttp2TranslatedLastHttpContent(ByteBuf content, int streamId) {
        this(content, streamId, true);
    }

    public DefaultHttp2TranslatedLastHttpContent(ByteBuf content, int streamId, boolean validateHeaders) {
        super(content, validateHeaders);
        this.streamId = streamId;
    }

    public int getStreamId() {
        return streamId;
    }

    @Override
    public DefaultHttp2TranslatedLastHttpContent replace(ByteBuf content) {
        DefaultHttp2TranslatedLastHttpContent dup = new DefaultHttp2TranslatedLastHttpContent(content, streamId);
        dup.trailingHeaders().set(trailingHeaders());
        return dup;
    }

    @Override
    public String toString() {
        return StringUtil.simpleClassName(this) +
                "(streamId: " + streamId + ", data: " + content() + ", decoderResult: " + decoderResult() + ')';
    }

    @Override
    public HttpHeaders trailingHeaders() {
        return super.trailingHeaders();
    }
}
