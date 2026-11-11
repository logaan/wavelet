Package "http:app@0.1.0"

Import {pkg: "wasi:http/types@0.2.0" as: http}
Import {pkg: "wasi:io/streams@0.2.0" as: streams}
Import {pkg: "http:greeting/api" as: greeting}

Def path-of Fn {request: borrow(incoming-request)}
  Match http/incoming-request-path-with-query(request)
    [(some(p)  p)
     (none     "/")]

Def html-escape Fn {text: string}
  Let {amp: join(split(text "&") "&amp;")
       lt: join(split(amp "<") "&lt;")}
    join(split(lt ">") "&gt;")

Def page Fn {path: string}
  str-cat(
    "<!doctype html>\n"
    "<title>http</title>\n"
    "<h1>" greeting/greet("world") "</h1>\n"
    "<p>You requested: " html-escape(path) "</p>\n"
  )

Def write-page Fn {body: borrow(outgoing-body), html: string}
  Match http/outgoing-body-write(body)
    [(ok(stream)
       Do [streams/blocking-write-and-flush(stream html)
           streams/drop-output-stream(stream)])
     (err() drop(0))]

Export {iface: "wasi:http/incoming-handler@0.2.0" name: handle
         params: {request: incoming-request response-out: response-outparam}}
Def handle Fn {request: incoming-request, response-out: response-outparam}
  Let {headers: http/fields()
        header-result: http/fields-append(headers "content-type" "text/html; charset=utf-8")
        response: http/outgoing-response(headers)
        path: path-of(request)
        dropped: http/drop-incoming-request(request)}
    Match http/outgoing-response-body(response)
      [(ok(body)
         Do [http/response-outparam-set(response-out ok(response))
             write-page(body page(path))
             http/outgoing-body-finish(body none)
             drop(0)])
       (err() drop(0))]
