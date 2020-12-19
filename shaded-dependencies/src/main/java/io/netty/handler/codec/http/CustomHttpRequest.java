/*
 * Copyright 2012 The Netty Project
 *
 * The Netty Project licenses this file to you under the Apache License,
 * version 2.0 (the "License"); you may not use this file except in compliance
 * with the License. You may obtain a copy of the License at:
 *
 *   https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS, WITHOUT
 * WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the
 * License for the specific language governing permissions and limitations
 * under the License.
 */
package io.netty.handler.codec.http;

import io.netty.util.internal.ObjectUtil;

public class CustomHttpRequest extends DefaultHttpMessage implements HttpRequest, HttpFrame {
    private static final int HASH_CODE_PRIME = 31;
    private HttpMethod method;
    private String uri;
    private Protocol protocol;
    private long id;

    public CustomHttpRequest(HttpVersion httpVersion, HttpMethod method, String uri, Protocol protocol, long id) {
        super(httpVersion, true, false);
        this.method = method;
        this.uri = uri;
        this.protocol = protocol;
        this.id = id;
    }

    @Override
    @Deprecated
    public HttpMethod getMethod() {
        return method();
    }

    @Override
    public HttpMethod method() {
        return method;
    }

    @Override
    @Deprecated
    public String getUri() {
        return uri();
    }

    @Override
    public String uri() {
        return uri;
    }

    @Override
    public HttpRequest setMethod(HttpMethod method) {
        this.method = ObjectUtil.checkNotNull(method, "method");
        return this;
    }

    @Override
    public HttpRequest setUri(String uri) {
        this.uri = ObjectUtil.checkNotNull(uri, "uri");
        return this;
    }

    @Override
    public HttpRequest setProtocolVersion(HttpVersion version) {
        super.setProtocolVersion(version);
        return this;
    }

    @Override
    public int hashCode() {
        int result = 1;
        result = HASH_CODE_PRIME * result + method.hashCode();
        result = HASH_CODE_PRIME * result + uri.hashCode();
        result = HASH_CODE_PRIME * result + super.hashCode();
        return result;
    }

    @Override
    public boolean equals(Object o) {
        if (!(o instanceof DefaultHttpRequest)) {
            return false;
        }

        DefaultHttpRequest other = (DefaultHttpRequest) o;

        return method().equals(other.method()) &&
                uri().equalsIgnoreCase(other.uri()) &&
                super.equals(o);
    }

    @Override
    public String toString() {
        return HttpMessageUtil.appendRequest(new StringBuilder(256), this).toString();
    }

    @Override
    public Protocol protocol() {
        return protocol;
    }

    @Override
    public void protocol(Protocol protocol) {
        this.protocol = protocol;
    }

    @Override
    public long id() {
        return id;
    }

    @Override
    public void id(long id) {
        this.id = id;
    }
}
