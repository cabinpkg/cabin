const MiniCssExtractPlugin = require("mini-css-extract-plugin");
const UglifyJsPlugin = require('uglifyjs-webpack-plugin');
const OptimizeCSSAssetsPlugin = require('optimize-css-assets-webpack-plugin');
const CopyPlugin = require('copy-webpack-plugin');
const AutoPrefixer = require('autoprefixer');

module.exports = (env, argv) => ({
  optimization: {
    minimizer: [
      new UglifyJsPlugin({
        cache: argv.mode === 'development',
        parallel: true,
        sourceMap: argv.mode === 'development'
      }),
      new OptimizeCSSAssetsPlugin({}),
    ]
  },

  entry: {
    index: ['babel-polyfill', './js/app.js'],
  },

  output: {
    path: `${__dirname}/dist/js`,
    filename: 'app.js',
  },

  plugins: [
    new MiniCssExtractPlugin({
      filename: '../css/style.css',
    }),
    new CopyPlugin({
      patterns: [
        { from: 'scss/colorize/dark.css', to: '../css/' },
        { from: 'scss/colorize/light.css', to: '../css/' },
        { from: 'assets/', to: '../' },
        { from: 'index.html', to: '../' },
      ],
    }),
  ],

  module: {
    rules: [
      {
        test: /\.js$/,
        exclude: /node_modules/,
        use: {
          loader: 'babel-loader',
        },
      },
      {
        test: /\.scss$/,
        exclude: [/node_modules/, /colorize/],
        use: [
          {
            loader: MiniCssExtractPlugin.loader,
          },
          {
            loader: "css-loader",
            options: {
              url: false,
              sourceMap: argv.mode === 'development',

              // 0 => no loaders (default);
              // 1 => postcss-loader;
              // 2 => postcss-loader, sass-loader
              importLoaders: 2,
            },
          },
          {
            loader: "postcss-loader",
            options: {
              plugins: [
                AutoPrefixer({ grid: true })
              ],
            },
          },
          {
            loader: "sass-loader",
            options: {
              sourceMap: argv.mode === 'development',
            },
          },
        ],
      },
      {
        test: /\.elm$/,
        exclude: [/elm-stuff/, /node_modules/],
        loader: "elm-webpack-loader",
        options: {
          optimize: argv.mode === 'production',
        },
      },
    ],
    noParse: [/\.elm$/],
  },
});
