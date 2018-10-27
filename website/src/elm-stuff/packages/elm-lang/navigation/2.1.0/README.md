# Navigation

Normally when the address bar changes, the browser sends some HTTP requests to load new pages.

This library lets you capture navigation and handle it yourself. No need to kick off a request to your servers.


## Examples

Check out the `examples/` directory of this repo. The `README` there will give you more information.


## Right and Middle Clicks on Hashless URLs

We got an issue asking "[what is the best way to handle clicks on *normal* URLs?](https://github.com/elm-lang/navigation/issues/13)" where you want to handle the navigation by hand, but also permit middle and right clicks.

The pending solution lives [here](https://github.com/elm-lang/html/issues/110).


## Context

You want your website to load quickly, especially if many users will be on mobile phones. You also want to send as little data as possible to users, especially if they have slow internet connections or data caps.

> For some reason, this general goal is called “Single Page Apps” or SPAs in the JavaScript world. It is odd in that the essence is that you want to manage multiple pages intelligently, and maybe “asset management” would have been clearer, but that ship has sailed.

One important trick is to use the browser cache as much as possible. Ideally, when a user navigates to a new page, they do not make *any* HTTP requests. They have it all already. In a good case, you have most of the JS code you need already, and you just ask for a tiny bit more.

Normally, browser navigation causes an HTTP request for some HTML. That HTML may then fetch some JavaScript and images and other stuff. By capturing navigation events (like this library does) you can skip that first HTTP request and figure out a way to get only the assets you are missing.
