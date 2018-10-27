module Html.Attributes exposing
  ( style, property, attribute, map
  , class, classList, id, title, hidden
  , type_, value, defaultValue, checked, placeholder, selected
  , accept, acceptCharset, action, autocomplete, autofocus
  , disabled, enctype, formaction, list, maxlength, minlength, method, multiple
  , name, novalidate, pattern, readonly, required, size, for, form
  , max, min, step
  , cols, rows, wrap
  , href, target, download, downloadAs, hreflang, media, ping, rel
  , ismap, usemap, shape, coords
  , src, height, width, alt
  , autoplay, controls, loop, preload, poster, default, kind, srclang
  , sandbox, seamless, srcdoc
  , reversed, start
  , align, colspan, rowspan, headers, scope
  , async, charset, content, defer, httpEquiv, language, scoped
  , accesskey, contenteditable, contextmenu, dir, draggable, dropzone
  , itemprop, lang, spellcheck, tabindex
  , challenge, keytype
  , cite, datetime, pubdate, manifest
  )

{-| Helper functions for HTML attributes. They are organized roughly by
category. Each attribute is labeled with the HTML tags it can be used with, so
just search the page for `video` if you want video stuff.

If you cannot find what you are looking for, go to the [Custom
Attributes](#custom-attributes) section to learn how to create new helpers.

# Primitives
@docs style, property, attribute, map

# Super Common Attributes
@docs class, classList, id, title, hidden

# Inputs
@docs type_, value, defaultValue, checked, placeholder, selected

## Input Helpers
@docs accept, acceptCharset, action, autocomplete, autofocus,
    disabled, enctype, formaction, list, maxlength, minlength, method, multiple,
    name, novalidate, pattern, readonly, required, size, for, form

## Input Ranges
@docs max, min, step

## Input Text Areas
@docs cols, rows, wrap


# Links and Areas
@docs href, target, download, downloadAs, hreflang, media, ping, rel

## Maps
@docs ismap, usemap, shape, coords


# Embedded Content
@docs src, height, width, alt

## Audio and Video
@docs autoplay, controls, loop, preload, poster, default, kind, srclang

## iframes
@docs sandbox, seamless, srcdoc

# Ordered Lists
@docs reversed, start

# Tables
@docs align, colspan, rowspan, headers, scope

# Header Stuff
@docs async, charset, content, defer, httpEquiv, language, scoped

# Less Common Global Attributes
Attributes that can be attached to any HTML tag but are less commonly used.
@docs accesskey, contenteditable, contextmenu, dir, draggable, dropzone,
      itemprop, lang, spellcheck, tabindex

# Key Generation
@docs challenge, keytype

# Miscellaneous
@docs cite, datetime, pubdate, manifest

-}

import Html exposing (Attribute)
import Json.Encode as Json
import VirtualDom


-- This library does not include low, high, or optimum because the idea of a
-- `meter` is just too crazy.



-- PRIMITIVES


{-| Specify a list of styles.

    myStyle : Attribute msg
    myStyle =
      style
        [ ("backgroundColor", "red")
        , ("height", "90px")
        , ("width", "100%")
        ]

    greeting : Html msg
    greeting =
      div [ myStyle ] [ text "Hello!" ]

There is no `Html.Styles` module because best practices for working with HTML
suggest that this should primarily be specified in CSS files. So the general
recommendation is to use this function lightly.
-}
style : List (String, String) -> Attribute msg
style =
  VirtualDom.style


{-| This function makes it easier to build a space-separated class attribute.
Each class can easily be added and removed depending on the boolean value it
is paired with. For example, maybe we want a way to view notices:

    viewNotice : Notice -> Html msg
    viewNotice notice =
      div
        [ classList
            [ ("notice", True)
            , ("notice-important", notice.isImportant)
            , ("notice-seen", notice.isSeen)
            ]
        ]
        [ text notice.content ]
-}
classList : List (String, Bool) -> Attribute msg
classList list =
  list
    |> List.filter Tuple.second
    |> List.map Tuple.first
    |> String.join " "
    |> class



-- CUSTOM ATTRIBUTES


{-| Create *properties*, like saying `domNode.className = 'greeting'` in
JavaScript.

    import Json.Encode as Encode

    class : String -> Attribute msg
    class name =
      property "className" (Encode.string name)

Read more about the difference between properties and attributes [here][].

[here]: https://github.com/elm-lang/html/blob/master/properties-vs-attributes.md
-}
property : String -> Json.Value -> Attribute msg
property =
  VirtualDom.property


stringProperty : String -> String -> Attribute msg
stringProperty name string =
  property name (Json.string string)


boolProperty : String -> Bool -> Attribute msg
boolProperty name bool =
  property name (Json.bool bool)


{-| Create *attributes*, like saying `domNode.setAttribute('class', 'greeting')`
in JavaScript.

    class : String -> Attribute msg
    class name =
      attribute "class" name

Read more about the difference between properties and attributes [here][].

[here]: https://github.com/elm-lang/html/blob/master/properties-vs-attributes.md
-}
attribute : String -> String -> Attribute msg
attribute =
  VirtualDom.attribute


{-| Transform the messages produced by an `Attribute`.
-}
map : (a -> msg) -> Attribute a -> Attribute msg
map =
  VirtualDom.mapProperty



-- GLOBAL ATTRIBUTES


{-| Often used with CSS to style elements with common properties. -}
class : String -> Attribute msg
class name =
  stringProperty "className" name


{-| Indicates the relevance of an element. -}
hidden : Bool -> Attribute msg
hidden bool =
  boolProperty "hidden" bool


{-| Often used with CSS to style a specific element. The value of this
attribute must be unique.
-}
id : String -> Attribute msg
id name =
  stringProperty "id" name


{-| Text to be displayed in a tooltip when hovering over the element. -}
title : String -> Attribute msg
title name =
  stringProperty "title" name



-- LESS COMMON GLOBAL ATTRIBUTES


{-| Defines a keyboard shortcut to activate or add focus to the element. -}
accesskey : Char -> Attribute msg
accesskey char =
  stringProperty "accessKey" (String.fromChar char)


{-| Indicates whether the element's content is editable. -}
contenteditable : Bool -> Attribute msg
contenteditable bool =
  boolProperty "contentEditable" bool


{-| Defines the ID of a `menu` element which will serve as the element's
context menu.
-}
contextmenu : String -> Attribute msg
contextmenu value =
  attribute "contextmenu" value


{-| Defines the text direction. Allowed values are ltr (Left-To-Right) or rtl
(Right-To-Left).
-}
dir : String -> Attribute msg
dir value =
  stringProperty "dir" value


{-| Defines whether the element can be dragged. -}
draggable : String -> Attribute msg
draggable value =
  attribute "draggable" value


{-| Indicates that the element accept the dropping of content on it. -}
dropzone : String -> Attribute msg
dropzone value =
  stringProperty "dropzone" value


{-|-}
itemprop : String -> Attribute msg
itemprop value =
  attribute "itemprop" value


{-| Defines the language used in the element. -}
lang : String -> Attribute msg
lang value =
  stringProperty "lang" value


{-| Indicates whether spell checking is allowed for the element. -}
spellcheck : Bool -> Attribute msg
spellcheck bool =
  boolProperty "spellcheck" bool


{-| Overrides the browser's default tab order and follows the one specified
instead.
-}
tabindex : Int -> Attribute msg
tabindex n =
  attribute "tabIndex" (toString n)



-- HEADER STUFF


{-| Indicates that the `script` should be executed asynchronously. -}
async : Bool -> Attribute msg
async bool =
  boolProperty "async" bool


{-| Declares the character encoding of the page or script. Common values include:

  * UTF-8 - Character encoding for Unicode
  * ISO-8859-1 - Character encoding for the Latin alphabet

For `meta` and `script`.
-}
charset : String -> Attribute msg
charset value =
  attribute "charset" value


{-| A value associated with http-equiv or name depending on the context. For
`meta`.
-}
content : String -> Attribute msg
content value =
  stringProperty "content" value


{-| Indicates that a `script` should be executed after the page has been
parsed.
-}
defer : Bool -> Attribute msg
defer bool =
  boolProperty "defer" bool


{-| This attribute is an indicator that is paired with the `content` attribute,
indicating what that content means. `httpEquiv` can take on three different
values: content-type, default-style, or refresh. For `meta`.
-}
httpEquiv : String -> Attribute msg
httpEquiv value =
  stringProperty "httpEquiv" value


{-| Defines the script language used in a `script`. -}
language : String -> Attribute msg
language value =
  stringProperty "language" value


{-| Indicates that a `style` should only apply to its parent and all of the
parents children.
-}
scoped : Bool -> Attribute msg
scoped bool =
  boolProperty "scoped" bool



-- EMBEDDED CONTENT


{-| The URL of the embeddable content. For `audio`, `embed`, `iframe`, `img`,
`input`, `script`, `source`, `track`, and `video`.
-}
src : String -> Attribute msg
src value =
  stringProperty "src" value


{-| Declare the height of a `canvas`, `embed`, `iframe`, `img`, `input`,
`object`, or `video`.
-}
height : Int -> Attribute msg
height value =
  attribute "height" (toString value)


{-| Declare the width of a `canvas`, `embed`, `iframe`, `img`, `input`,
`object`, or `video`.
-}
width : Int -> Attribute msg
width value =
  attribute "width" (toString value)


{-| Alternative text in case an image can't be displayed. Works with `img`,
`area`, and `input`.
-}
alt : String -> Attribute msg
alt value =
  stringProperty "alt" value



-- AUDIO and VIDEO


{-| The `audio` or `video` should play as soon as possible. -}
autoplay : Bool -> Attribute msg
autoplay bool =
  boolProperty "autoplay" bool


{-| Indicates whether the browser should show playback controls for the `audio`
or `video`.
-}
controls : Bool -> Attribute msg
controls bool =
  boolProperty "controls" bool


{-| Indicates whether the `audio` or `video` should start playing from the
start when it's finished.
-}
loop : Bool -> Attribute msg
loop bool =
  boolProperty "loop" bool


{-| Control how much of an `audio` or `video` resource should be preloaded. -}
preload : String -> Attribute msg
preload value =
  stringProperty "preload" value


{-| A URL indicating a poster frame to show until the user plays or seeks the
`video`.
-}
poster : String -> Attribute msg
poster value =
  stringProperty "poster" value


{-| Indicates that the `track` should be enabled unless the user's preferences
indicate something different.
-}
default : Bool -> Attribute msg
default bool =
  boolProperty "default" bool


{-| Specifies the kind of text `track`. -}
kind : String -> Attribute msg
kind value =
  stringProperty "kind" value


{-- TODO: maybe reintroduce once there's a better way to disambiguate imports
{-| Specifies a user-readable title of the text `track`. -}
label : String -> Attribute msg
label value =
  stringProperty "label" value
--}

{-| A two letter language code indicating the language of the `track` text data.
-}
srclang : String -> Attribute msg
srclang value =
  stringProperty "srclang" value



-- IFRAMES


{-| A space separated list of security restrictions you'd like to lift for an
`iframe`.
-}
sandbox : String -> Attribute msg
sandbox value =
  stringProperty "sandbox" value


{-|  Make an `iframe` look like part of the containing document. -}
seamless : Bool -> Attribute msg
seamless bool =
  boolProperty "seamless" bool


{-| An HTML document that will be displayed as the body of an `iframe`. It will
override the content of the `src` attribute if it has been specified.
-}
srcdoc : String -> Attribute msg
srcdoc value =
  stringProperty "srcdoc" value



-- INPUT


{-| Defines the type of a `button`, `input`, `embed`, `object`, `script`,
`source`, `style`, or `menu`.
-}
type_ : String -> Attribute msg
type_ value =
  stringProperty "type" value


{-| Defines a default value which will be displayed in a `button`, `option`,
`input`, `li`, `meter`, `progress`, or `param`.
-}
value : String -> Attribute msg
value value =
  stringProperty "value" value


{-| Defines an initial value which will be displayed in an `input` when that
`input` is added to the DOM. Unlike `value`, altering `defaultValue` after the
`input` element has been added to the DOM has no effect.
-}
defaultValue : String -> Attribute msg
defaultValue value =
  stringProperty "defaultValue" value


{-| Indicates whether an `input` of type checkbox is checked. -}
checked : Bool -> Attribute msg
checked bool =
  boolProperty "checked" bool


{-| Provides a hint to the user of what can be entered into an `input` or
`textarea`.
-}
placeholder : String -> Attribute msg
placeholder value =
  stringProperty "placeholder" value


{-| Defines which `option` will be selected on page load. -}
selected : Bool -> Attribute msg
selected bool =
  boolProperty "selected" bool



-- INPUT HELPERS


{-| List of types the server accepts, typically a file type.
For `form` and `input`.
-}
accept : String -> Attribute msg
accept value =
  stringProperty "accept" value


{-| List of supported charsets in a `form`.
-}
acceptCharset : String -> Attribute msg
acceptCharset value =
  stringProperty "acceptCharset" value


{-| The URI of a program that processes the information submitted via a `form`.
-}
action : String -> Attribute msg
action value =
  stringProperty "action" value


{-| Indicates whether a `form` or an `input` can have their values automatically
completed by the browser.
-}
autocomplete : Bool -> Attribute msg
autocomplete bool =
  stringProperty "autocomplete" (if bool then "on" else "off")


{-| The element should be automatically focused after the page loaded.
For `button`, `input`, `keygen`, `select`, and `textarea`.
-}
autofocus : Bool -> Attribute msg
autofocus bool =
  boolProperty "autofocus" bool


{-| Indicates whether the user can interact with a `button`, `fieldset`,
`input`, `keygen`, `optgroup`, `option`, `select` or `textarea`.
-}
disabled : Bool -> Attribute msg
disabled bool =
  boolProperty "disabled" bool


{-| How `form` data should be encoded when submitted with the POST method.
Options include: application/x-www-form-urlencoded, multipart/form-data, and
text/plain.
-}
enctype : String -> Attribute msg
enctype value =
  stringProperty "enctype" value


{-| Indicates the action of an `input` or `button`. This overrides the action
defined in the surrounding `form`.
-}
formaction : String -> Attribute msg
formaction value =
  attribute "formAction" value


{-| Associates an `input` with a `datalist` tag. The datalist gives some
pre-defined options to suggest to the user as they interact with an input.
The value of the list attribute must match the id of a `datalist` node.
For `input`.
-}
list : String -> Attribute msg
list value =
  attribute "list" value


{-| Defines the minimum number of characters allowed in an `input` or
`textarea`.
-}
minlength : Int -> Attribute msg
minlength n =
  attribute "minLength" (toString n)


{-| Defines the maximum number of characters allowed in an `input` or
`textarea`.
-}
maxlength : Int -> Attribute msg
maxlength n =
  attribute "maxlength" (toString n)


{-| Defines which HTTP method to use when submitting a `form`. Can be GET
(default) or POST.
-}
method : String -> Attribute msg
method value =
  stringProperty "method" value


{-| Indicates whether multiple values can be entered in an `input` of type
email or file. Can also indicate that you can `select` many options.
-}
multiple : Bool -> Attribute msg
multiple bool =
  boolProperty "multiple" bool


{-| Name of the element. For example used by the server to identify the fields
in form submits. For `button`, `form`, `fieldset`, `iframe`, `input`, `keygen`,
`object`, `output`, `select`, `textarea`, `map`, `meta`, and `param`.
-}
name : String -> Attribute msg
name value =
  stringProperty "name" value


{-| This attribute indicates that a `form` shouldn't be validated when
submitted.
-}
novalidate : Bool -> Attribute msg
novalidate bool =
  boolProperty "noValidate" bool


{-| Defines a regular expression which an `input`'s value will be validated
against.
-}
pattern : String -> Attribute msg
pattern value =
  stringProperty "pattern" value


{-| Indicates whether an `input` or `textarea` can be edited. -}
readonly : Bool -> Attribute msg
readonly bool =
  boolProperty "readOnly" bool


{-| Indicates whether this element is required to fill out or not.
For `input`, `select`, and `textarea`.
-}
required : Bool -> Attribute msg
required bool =
  boolProperty "required" bool


{-| For `input` specifies the width of an input in characters.

For `select` specifies the number of visible options in a drop-down list.
-}
size : Int -> Attribute msg
size n =
  attribute "size" (toString n)


{-| The element ID described by this `label` or the element IDs that are used
for an `output`.
-}
for : String -> Attribute msg
for value =
  stringProperty "htmlFor" value


{-| Indicates the element ID of the `form` that owns this particular `button`,
`fieldset`, `input`, `keygen`, `label`, `meter`, `object`, `output`,
`progress`, `select`, or `textarea`.
-}
form : String -> Attribute msg
form value =
  attribute "form" value



-- RANGES


{-| Indicates the maximum value allowed. When using an input of type number or
date, the max value must be a number or date. For `input`, `meter`, and `progress`.
-}
max : String -> Attribute msg
max value =
  stringProperty "max" value


{-| Indicates the minimum value allowed. When using an input of type number or
date, the min value must be a number or date. For `input` and `meter`.
-}
min : String -> Attribute msg
min value =
  stringProperty "min" value


{-| Add a step size to an `input`. Use `step "any"` to allow any floating-point
number to be used in the input.
-}
step : String -> Attribute msg
step n =
  stringProperty "step" n


--------------------------


{-| Defines the number of columns in a `textarea`. -}
cols : Int -> Attribute msg
cols n =
  attribute "cols" (toString n)


{-| Defines the number of rows in a `textarea`. -}
rows : Int -> Attribute msg
rows n =
  attribute "rows" (toString n)


{-| Indicates whether the text should be wrapped in a `textarea`. Possible
values are "hard" and "soft".
-}
wrap : String -> Attribute msg
wrap value =
  stringProperty "wrap" value



-- MAPS


{-| When an `img` is a descendent of an `a` tag, the `ismap` attribute
indicates that the click location should be added to the parent `a`'s href as
a query string.
-}
ismap : Bool -> Attribute msg
ismap value =
  boolProperty "isMap" value


{-| Specify the hash name reference of a `map` that should be used for an `img`
or `object`. A hash name reference is a hash symbol followed by the element's name or id.
E.g. `"#planet-map"`.
-}
usemap : String -> Attribute msg
usemap value =
  stringProperty "useMap" value


{-| Declare the shape of the clickable area in an `a` or `area`. Valid values
include: default, rect, circle, poly. This attribute can be paired with
`coords` to create more particular shapes.
-}
shape : String -> Attribute msg
shape value =
  stringProperty "shape" value


{-| A set of values specifying the coordinates of the hot-spot region in an
`area`. Needs to be paired with a `shape` attribute to be meaningful.
-}
coords : String -> Attribute msg
coords value =
  stringProperty "coords" value



-- KEY GEN


{-| A challenge string that is submitted along with the public key in a `keygen`.
-}
challenge : String -> Attribute msg
challenge value =
  attribute "challenge" value


{-| Specifies the type of key generated by a `keygen`. Possible values are:
rsa, dsa, and ec.
-}
keytype : String -> Attribute msg
keytype value =
  stringProperty "keytype" value



-- REAL STUFF


{-| Specifies the horizontal alignment of a `caption`, `col`, `colgroup`,
`hr`, `iframe`, `img`, `table`, `tbody`,  `td`,  `tfoot`, `th`, `thead`, or
`tr`.
-}
align : String -> Attribute msg
align value =
  stringProperty "align" value


{-| Contains a URI which points to the source of the quote or change in a
`blockquote`, `del`, `ins`, or `q`.
-}
cite : String -> Attribute msg
cite value =
  stringProperty "cite" value




-- LINKS AND AREAS


{-| The URL of a linked resource, such as `a`, `area`, `base`, or `link`. -}
href : String -> Attribute msg
href value =
  stringProperty "href" value


{-| Specify where the results of clicking an `a`, `area`, `base`, or `form`
should appear. Possible special values include:

  * _blank &mdash; a new window or tab
  * _self &mdash; the same frame (this is default)
  * _parent &mdash; the parent frame
  * _top &mdash; the full body of the window

You can also give the name of any `frame` you have created.
-}
target : String -> Attribute msg
target value =
  stringProperty "target" value


{-| Indicates that clicking an `a` and `area` will download the resource
directly.
-}
download : Bool -> Attribute msg
download bool =
  boolProperty "download" bool


{-| Indicates that clicking an `a` and `area` will download the resource
directly, and that the downloaded resource with have the given filename.
-}
downloadAs : String -> Attribute msg
downloadAs value =
  stringProperty "download" value


{-| Two-letter language code of the linked resource of an `a`, `area`, or `link`.
-}
hreflang : String -> Attribute msg
hreflang value =
  stringProperty "hreflang" value


{-| Specifies a hint of the target media of a `a`, `area`, `link`, `source`,
or `style`.
-}
media : String -> Attribute msg
media value =
  attribute "media" value


{-| Specify a URL to send a short POST request to when the user clicks on an
`a` or `area`. Useful for monitoring and tracking.
-}
ping : String -> Attribute msg
ping value =
  stringProperty "ping" value


{-| Specifies the relationship of the target object to the link object.
For `a`, `area`, `link`.
-}
rel : String -> Attribute msg
rel value =
  attribute "rel" value



-- CRAZY STUFF


{-| Indicates the date and time associated with the element.
For `del`, `ins`, `time`.
-}
datetime : String -> Attribute msg
datetime value =
  attribute "datetime" value


{-| Indicates whether this date and time is the date of the nearest `article`
ancestor element. For `time`.
-}
pubdate : String -> Attribute msg
pubdate value =
  attribute "pubdate" value



-- ORDERED LISTS


{-| Indicates whether an ordered list `ol` should be displayed in a descending
order instead of a ascending.
-}
reversed : Bool -> Attribute msg
reversed bool =
  boolProperty "reversed" bool


{-| Defines the first number of an ordered list if you want it to be something
besides 1.
-}
start : Int -> Attribute msg
start n =
  stringProperty "start" (toString n)



-- TABLES


{-| The colspan attribute defines the number of columns a cell should span.
For `td` and `th`.
-}
colspan : Int -> Attribute msg
colspan n =
  attribute "colspan" (toString n)


{-| A space separated list of element IDs indicating which `th` elements are
headers for this cell. For `td` and `th`.
-}
headers : String -> Attribute msg
headers value =
  stringProperty "headers" value


{-| Defines the number of rows a table cell should span over.
For `td` and `th`.
-}
rowspan : Int -> Attribute msg
rowspan n =
  attribute "rowspan" (toString n)


{-| Specifies the scope of a header cell `th`. Possible values are: col, row,
colgroup, rowgroup.
-}
scope : String -> Attribute msg
scope value =
  stringProperty "scope" value


{-| Specifies the URL of the cache manifest for an `html` tag. -}
manifest : String -> Attribute msg
manifest value =
  attribute "manifest" value


{-- TODO: maybe reintroduce once there's a better way to disambiguate imports
{-| The number of columns a `col` or `colgroup` should span. -}
span : Int -> Attribute msg
span n =
    stringProperty "span" (toString n)
--}
